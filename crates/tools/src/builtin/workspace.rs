//! Local workspace tools, confined to the session's `workspace_root`.
//!
//! Every path is resolved against the canonical root and re-checked after resolving symlinks,
//! so `..`, absolute paths and links pointing outside are rejected. Writes never follow a
//! symlink. `run_command` runs in its own process group, which is killed on timeout and on
//! cancellation (interrupt / stop of the turn).

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oort_agent::{LocalTool, LocalToolExecutor, ToolContext, ToolError, ToolOutput};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::{BuiltinContext, missing, str_arg, tool_def, trim_middle};

const MAX_READ_BYTES: u64 = 256 * 1024;

/// `run_command` limits.
#[derive(Debug, Clone)]
pub struct CommandOptions {
    pub default_timeout: Duration,
    pub max_timeout: Duration,
    pub max_output_chars: usize,
}

impl Default for CommandOptions {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(120),
            max_timeout: Duration::from_secs(600),
            max_output_chars: 30_000,
        }
    }
}

fn outside(rel: &str) -> ToolError {
    ToolError::InvalidArguments(format!("{rel} is outside the workspace"))
}

/// An existing path inside `root` (symlinks resolved, then checked).
pub fn resolve_existing(root: &Path, rel: &str) -> Result<PathBuf, ToolError> {
    let canon = root
        .join(rel)
        .canonicalize()
        .map_err(|e| ToolError::Failed(format!("{rel}: {e}")))?;
    if !canon.starts_with(root) {
        return Err(outside(rel));
    }
    Ok(canon)
}

/// A path to write inside `root`: the nearest existing ancestor must resolve inside the root,
/// the remainder may not contain `..`, and an existing target may not be a symlink.
pub fn resolve_for_write(root: &Path, rel: &str) -> Result<PathBuf, ToolError> {
    let joined = root.join(rel);
    if joined.file_name().is_none() {
        return Err(ToolError::InvalidArguments(format!(
            "{rel} is not a file path"
        )));
    }
    let mut existing = joined.as_path();
    let mut rest: Vec<Component> = Vec::new();
    while std::fs::symlink_metadata(existing).is_err() {
        let Some(parent) = existing.parent() else {
            return Err(outside(rel));
        };
        rest.push(
            existing
                .components()
                .next_back()
                .ok_or_else(|| outside(rel))?,
        );
        existing = parent;
    }
    if rest.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(outside(rel));
    }
    let meta = std::fs::symlink_metadata(existing).map_err(|e| ToolError::Failed(e.to_string()))?;
    if rest.is_empty() && meta.file_type().is_symlink() {
        return Err(ToolError::InvalidArguments(format!(
            "{rel} is a symbolic link; refusing to write through it"
        )));
    }
    let base = existing
        .canonicalize()
        .map_err(|e| ToolError::Failed(e.to_string()))?;
    if !base.starts_with(root) {
        return Err(outside(rel));
    }
    let mut out = base;
    for c in rest.iter().rev() {
        out.push(c.as_os_str());
    }
    Ok(out)
}

fn root(ctx: &BuiltinContext) -> Result<&Path, ToolError> {
    ctx.workspace_root
        .as_deref()
        .ok_or_else(|| missing("the workspace"))
}

// ----- read_file -----------------------------------------------------------------------------

pub struct ReadFile {
    ctx: BuiltinContext,
}

tool_def!(ReadFile, "read_file", true,
"Read a text file in the workspace. Lines are numbered; use offset/limit for large files.",
{
    "type": "object",
    "properties": {
        "path": { "type": "string", "description": "relative to the workspace root" },
        "offset": { "type": "integer", "minimum": 1, "description": "first line (1-based)" },
        "limit": { "type": "integer", "minimum": 1, "description": "number of lines" }
    },
    "required": ["path"]
});

#[async_trait]
impl LocalTool for ReadFile {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let rel = str_arg(&args, "path")?;
        let path = resolve_existing(root(&self.ctx)?, rel)?;
        let mut f = tokio::fs::File::open(&path)
            .await
            .map_err(|e| ToolError::Failed(format!("{rel}: {e}")))?;
        let size = f.metadata().await.map(|m| m.len()).unwrap_or(0);
        let mut buf = Vec::new();
        (&mut f)
            .take(MAX_READ_BYTES)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        if buf.iter().take(8192).any(|b| *b == 0) {
            return Err(ToolError::Failed(format!("{rel} looks like a binary file")));
        }
        let text = String::from_utf8_lossy(&buf);
        let offset = args
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(2000) as usize;
        let lines: Vec<&str> = text.lines().collect();
        let mut out = String::new();
        for (i, l) in lines.iter().enumerate().skip(offset - 1).take(limit) {
            out.push_str(&format!("{:>6}\t{l}\n", i + 1));
        }
        let shown_end = (offset - 1 + limit).min(lines.len());
        if shown_end < lines.len() || size > MAX_READ_BYTES {
            out.push_str(&format!(
                "[showing lines {offset}-{shown_end}{}; use offset/limit for more]\n",
                if size > MAX_READ_BYTES {
                    format!(" of the first {} KiB", MAX_READ_BYTES / 1024)
                } else {
                    format!(" of {}", lines.len())
                }
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

// ----- write_file ----------------------------------------------------------------------------

pub struct WriteFile {
    ctx: BuiltinContext,
}

tool_def!(WriteFile, "write_file", false,
"Create or overwrite a file in the workspace (`content`), or replace one exact occurrence of `old_string` with `new_string` (set `replace_all` for every occurrence). The user sees a diff before it is applied.",
{
    "type": "object",
    "properties": {
        "path": { "type": "string" },
        "content": { "type": "string" },
        "old_string": { "type": "string" },
        "new_string": { "type": "string" },
        "replace_all": { "type": "boolean" }
    },
    "required": ["path"]
});

/// `(path, old, new)` for a write: the new content is computed without touching the disk.
async fn plan_write(args: &Value, root: &Path) -> Result<(PathBuf, String, String), ToolError> {
    let rel = str_arg(args, "path")?;
    let path = resolve_for_write(root, rel)?;
    let old = match tokio::fs::read(&path).await {
        Ok(b) if b.iter().take(8192).any(|x| *x == 0) => {
            return Err(ToolError::Failed(format!("{rel} is a binary file")));
        }
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ToolError::Failed(format!("{rel}: {e}"))),
    };
    let new = match (
        args.get("content").and_then(Value::as_str),
        args.get("old_string").and_then(Value::as_str),
    ) {
        (Some(c), None) => c.to_string(),
        (None, Some(from)) => {
            let to = str_arg(args, "new_string")?;
            let n = old.matches(from).count();
            let all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match (n, all) {
                (0, _) => {
                    return Err(ToolError::InvalidArguments(format!(
                        "old_string not found in {rel}"
                    )));
                }
                (1, _) | (_, true) => old.replace(from, to),
                _ => {
                    return Err(ToolError::InvalidArguments(format!(
                        "old_string occurs {n} times in {rel}; add context or set replace_all"
                    )));
                }
            }
        }
        _ => {
            return Err(ToolError::InvalidArguments(
                "pass either `content` or `old_string` + `new_string`".into(),
            ));
        }
    };
    Ok((path, old, new))
}

pub fn unified_diff(rel: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{rel}"), &format!("b/{rel}"))
        .to_string()
}

#[async_trait]
impl LocalTool for WriteFile {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let root = root(&self.ctx)?;
        let (path, old, new) = plan_write(&args, root).await?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
        }
        // Re-check after creating directories (a racing symlink must not redirect the write).
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        let path = resolve_for_write(root, &rel)?;
        tokio::fs::write(&path, &new)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let diff = unified_diff(&rel, &old, &new);
        let (add, del) = diff.lines().fold((0, 0), |(a, d), l| {
            if l.starts_with('+') && !l.starts_with("+++") {
                (a + 1, d)
            } else if l.starts_with('-') && !l.starts_with("---") {
                (a, d + 1)
            } else {
                (a, d)
            }
        });
        Ok(ToolOutput::ok(format!("Wrote {rel} (+{add} −{del}).")))
    }
}

/// Approval preview: the diff the write would produce.
pub async fn write_preview(args: &Value, ctx: &BuiltinContext) -> Option<Value> {
    let root = ctx.workspace_root.as_deref()?;
    let rel = args.get("path")?.as_str()?;
    Some(match plan_write(args, root).await {
        Ok((path, old, new)) => json!({
            "kind": "diff",
            "path": rel,
            "created": !path.exists(),
            "diff": unified_diff(rel, &old, &new),
        }),
        Err(e) => json!({ "kind": "error", "path": rel, "error": e.to_string() }),
    })
}

// ----- run_command ---------------------------------------------------------------------------

pub struct RunCommand {
    ctx: BuiltinContext,
}

tool_def!(RunCommand, "run_command", false,
"Run a shell command in the workspace root and return exit_code, stdout and stderr. Always asks the user first.",
{
    "type": "object",
    "properties": {
        "command": { "type": "string" },
        "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 600 }
    },
    "required": ["command"]
});

fn kill_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: plain syscall; the group was created for this child (`process_group(0)`).
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

impl RunCommand {
    fn timeout(&self, args: &Value) -> Duration {
        args.get("timeout_secs")
            .and_then(Value::as_u64)
            .map(Duration::from_secs)
            .unwrap_or(self.ctx.commands.default_timeout)
            .min(self.ctx.commands.max_timeout)
    }
}

#[async_trait]
impl LocalTool for RunCommand {
    async fn run(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let root = root(&self.ctx)?;
        let command = str_arg(&args, "command")?;
        let timeout = self.timeout(&args);
        #[cfg(unix)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(command).process_group(0);
            c
        };
        #[cfg(not(unix))]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(command);
            c
        };
        cmd.current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Failed(format!("spawn: {e}")))?;
        let pid = child.id();
        let mut out = child.stdout.take().expect("piped");
        let mut err = child.stderr.take().expect("piped");
        let read_out = tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b).await;
            b
        });
        let read_err = tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b).await;
            b
        });
        let (status, timed_out) = tokio::select! {
            s = child.wait() => (s.ok(), false),
            _ = tokio::time::sleep(timeout) => {
                kill_group(pid);
                let _ = child.kill().await;
                (None, true)
            }
            _ = ctx.cancel.cancelled() => {
                kill_group(pid);
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(ToolError::Cancelled);
            }
        };
        // Children of the shell may still hold the pipes; the group is gone after a kill.
        if timed_out {
            let _ = child.wait().await;
        }
        let max = self.ctx.commands.max_output_chars;
        let grab = |h: tokio::task::JoinHandle<Vec<u8>>| async move {
            match tokio::time::timeout(Duration::from_secs(2), h).await {
                Ok(Ok(b)) => trim_middle(&String::from_utf8_lossy(&b), max),
                _ => String::new(),
            }
        };
        let stdout = grab(read_out).await;
        let stderr = grab(read_err).await;
        let exit_code = status.and_then(|s| s.code());
        let body = json!({
            "exit_code": exit_code,
            "timed_out": timed_out,
            "stdout": stdout,
            "stderr": stderr,
        });
        let text = serde_json::to_string_pretty(&body).unwrap_or_default();
        Ok(if timed_out || exit_code != Some(0) {
            ToolOutput::error(if timed_out {
                format!(
                    "Timed out after {}s (process group killed).\n{text}",
                    timeout.as_secs()
                )
            } else {
                text
            })
        } else {
            ToolOutput::ok(text)
        })
    }
}

/// Approval preview: the command, where it runs and for how long at most.
pub fn command_preview(args: &Value, ctx: &BuiltinContext) -> Option<Value> {
    let cmd = args.get("command")?.as_str()?;
    let opts = &ctx.commands;
    let timeout = args
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or(opts.default_timeout)
        .min(opts.max_timeout);
    Some(json!({
        "kind": "command",
        "command": cmd,
        "cwd": ctx.workspace_root.as_ref().map(|p| p.display().to_string()),
        "timeout_secs": timeout.as_secs(),
    }))
}

pub(super) fn add(ex: LocalToolExecutor, ctx: &BuiltinContext) -> LocalToolExecutor {
    ex.with_tool(Arc::new(ReadFile { ctx: ctx.clone() }))
        .with_tool(Arc::new(WriteFile { ctx: ctx.clone() }))
        .with_tool(Arc::new(RunCommand { ctx: ctx.clone() }))
}
