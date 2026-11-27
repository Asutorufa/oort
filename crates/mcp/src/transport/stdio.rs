//! stdio transport: a local MCP server subprocess speaking newline-delimited JSON-RPC.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

use super::{McpTransport, TransportError};
use crate::protocol::{self, Incoming, RpcError};

/// How to launch a stdio MCP server.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StdioServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

type Pending = Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// Owns the child process. Dropping it kills the process (`kill_on_drop`).
pub struct StdioTransport {
    name: String,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Pending,
    child: Mutex<Option<Child>>,
    tasks: Vec<JoinHandle<()>>,
}

/// Removes a pending entry when the request future is dropped (timeout / cancel).
struct PendingGuard {
    pending: Pending,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let (pending, id) = (self.pending.clone(), self.id);
        if let Ok(mut map) = pending.try_lock() {
            map.remove(&id);
        } else {
            tokio::spawn(async move {
                pending.lock().await.remove(&id);
            });
        }
    }
}

impl StdioTransport {
    /// Spawns the server. `name` is used for logs.
    pub fn spawn(name: impl Into<String>, cfg: &StdioServerConfig) -> Result<Self, TransportError> {
        let name = name.into();
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .envs(&cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &cfg.cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Io(format!("spawn `{}`: {e}", cfg.command)))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let pending: Pending = Arc::default();

        let reader = tokio::spawn(read_loop(
            name.clone(),
            stdout,
            pending.clone(),
            stdin.clone(),
        ));
        let log_name = name.clone();
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(server = %log_name, "stderr: {line}");
            }
        });
        Ok(Self {
            name,
            stdin,
            pending,
            child: Mutex::new(Some(child)),
            tasks: vec![reader, stderr_task],
        })
    }

    async fn write(&self, msg: &Value) -> Result<(), TransportError> {
        write_line(&self.stdin, msg).await
    }
}

async fn write_line(stdin: &Mutex<Option<ChildStdin>>, msg: &Value) -> Result<(), TransportError> {
    let mut guard = stdin.lock().await;
    let pipe = guard
        .as_mut()
        .ok_or_else(|| TransportError::Disconnected("stdin closed".into()))?;
    let mut line = serde_json::to_vec(msg).map_err(|e| TransportError::Protocol(e.to_string()))?;
    line.push(b'\n');
    pipe.write_all(&line)
        .await
        .map_err(|e| TransportError::Disconnected(format!("write failed: {e}")))?;
    pipe.flush()
        .await
        .map_err(|e| TransportError::Disconnected(format!("flush failed: {e}")))
}

async fn read_loop(
    name: String,
    stdout: tokio::process::ChildStdout,
    pending: Pending,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                tracing::error!(server = %name, error = %e, "stdout read failed");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            tracing::warn!(server = %name, "ignoring non-JSON line from server: {line}");
            continue;
        };
        match Incoming::parse(&value) {
            Some(Incoming::Response { id, result }) => {
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
            }
            Some(Incoming::Request { id, method }) => {
                let reply = protocol::reply_to_server_request(&id, &method);
                if let Err(e) = write_line(&stdin, &reply).await {
                    tracing::warn!(server = %name, error = %e, "failed to answer server request");
                }
            }
            Some(Incoming::Notification { method }) => {
                tracing::debug!(server = %name, %method, "server notification");
            }
            None => tracing::warn!(server = %name, "ignoring malformed JSON-RPC message"),
        }
    }
    // EOF: fail every waiter (dropping the senders) so no request hangs forever.
    pending.lock().await.clear();
    tracing::info!(server = %name, "server stdout closed");
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Result<Value, RpcError>, TransportError> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let _guard = PendingGuard {
            pending: self.pending.clone(),
            id,
        };
        self.write(&protocol::request(id, method, params)).await?;
        rx.await
            .map_err(|_| TransportError::Disconnected(format!("server `{}` exited", self.name)))
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError> {
        self.write(&protocol::notification(method, params)).await
    }

    /// Closes stdin (the MCP shutdown signal), waits briefly, then kills.
    async fn close(&self) {
        self.stdin.lock().await.take();
        if let Some(mut child) = self.child.lock().await.take() {
            match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(server = %self.name, "server did not exit; killing");
                    let _ = child.kill().await;
                }
            }
        }
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}
