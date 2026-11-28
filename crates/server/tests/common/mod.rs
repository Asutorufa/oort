//! Shared harness for the integration tests added with skills, widgets and OAuth.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use oort_server::{Registry, SessionManager, router};
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

pub struct Server {
    pub base: String,
    pub origin: String,
    pub manager: Arc<SessionManager>,
    pub store: Arc<SqliteSessionStore>,
    task: tokio::task::JoinHandle<()>,
}

pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oort-it-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

impl Server {
    pub async fn start(
        store: Arc<SqliteSessionStore>,
        workspace: &Path,
        skill_dirs: Vec<PathBuf>,
    ) -> Self {
        Self::start_with(store, workspace, skill_dirs, |r| r).await
    }

    /// Like [`Server::start`], with extra registry settings.
    pub async fn start_with(
        store: Arc<SqliteSessionStore>,
        workspace: &Path,
        skill_dirs: Vec<PathBuf>,
        configure: impl FnOnce(Registry) -> Registry,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let builtin = Arc::new(oort_server::tools::builtin_executor(workspace));
        let registry = Arc::new(configure(
            Registry::new(store.clone(), builtin)
                .with_mock_word_delay(Duration::from_millis(2))
                .with_skill_dirs(skill_dirs)
                .with_public_url(&origin),
        ));
        let manager = SessionManager::new(store.clone(), registry);
        let app = router(manager.clone(), None);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base: format!("{origin}/api"),
            origin,
            manager,
            store,
            task,
        }
    }

    pub async fn stop(self) {
        self.manager.shutdown().await;
        self.task.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    pub async fn send(&self, method: &str, path: &str, body: Option<Value>) -> reqwest::Response {
        let mut req =
            reqwest::Client::new().request(method.parse().unwrap(), format!("{}{path}", self.base));
        if let Some(b) = body {
            req = req.json(&b);
        }
        req.send().await.unwrap()
    }

    pub async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let r = self.send(method, path, body).await;
        let status = r.status();
        let text = r.text().await.unwrap();
        assert!(status.is_success(), "{method} {path}: {status} {text}");
        if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap()
        }
    }

    pub async fn get(&self, path: &str) -> Value {
        self.ok("GET", path, None).await
    }

    pub async fn create(&self, body: Value) -> String {
        self.ok("POST", "/sessions", Some(body)).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub async fn steer(&self, id: &str, text: &str) {
        let r = self
            .send(
                "POST",
                &format!("/sessions/{id}/input"),
                Some(json!({ "type": "steer", "text": text })),
            )
            .await;
        assert_eq!(r.status(), 202, "{}", r.text().await.unwrap());
    }

    pub async fn wait_for(&self, id: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..250 {
            let d = self.get(&format!("/sessions/{id}")).await;
            if pred(&d) {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "condition not reached: {}",
            self.get(&format!("/sessions/{id}")).await
        );
    }
}

pub fn status(d: &Value) -> &str {
    d["status"].as_str().unwrap()
}

pub fn raw_len(d: &Value) -> usize {
    d["raw"].as_array().unwrap().len()
}

pub fn tool_names(d: &Value) -> Vec<String> {
    d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// Tool results of the history, in order.
pub fn results(d: &Value) -> Vec<Value> {
    d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .flat_map(|m| m["content"].as_array().unwrap().clone())
        .collect()
}

pub async fn read_sse(
    resp: reqwest::Response,
    mut done: impl FnMut(&[Value]) -> bool,
) -> Vec<Value> {
    let mut body = resp.bytes_stream();
    let (mut buf, mut events) = (String::new(), Vec::new());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !done(&events) && tokio::time::Instant::now() < deadline {
        let Ok(Some(chunk)) = tokio::time::timeout(Duration::from_secs(5), body.next()).await
        else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            for line in frame.lines() {
                if let Some(d) = line.strip_prefix("data: ") {
                    events.push(serde_json::from_str(d).unwrap());
                }
            }
        }
    }
    events
}
