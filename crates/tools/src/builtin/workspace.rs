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

use super::output::{SPILL_THRESHOLD_CHARS, spill_if_needed};
use super::{BuiltinContext, missing, str_arg, tool_def, trim_middle};

const MAX_FILE_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_READ_LIMIT: usize = 200;
const DEFAULT_READ_MAX_BYTES: usize = 32 * 1024;
const HARD_READ_MAX_BYTES: usize = 256 * 1024;

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
"Read a text file in the workspace. Lines are numbered; use offset/limit/max_bytes for pagination, or search_in_file to jump to specific content.",
{
    "type": "object",
    "properties": {
        "path": { "type": "string", "description": "relative to the workspace root" },
        "offset": { "type": "integer", "minimum": 1, "description": "first line (1-based, default 1)" },
        "limit": { "type": "integer", "minimum": 1, "description": "maximum number of lines (default 200)" },
        "max_bytes": { "type": "integer", "minimum": 256, "maximum": 262144, "description": "maximum output bytes per call (default 32768)" }
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
            .take(MAX_FILE_INDEX_BYTES)
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
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_READ_LIMIT, |v| (v as usize).max(1));
        let max_bytes = args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_READ_MAX_BYTES, |v| {
                (v as usize).clamp(256, HARD_READ_MAX_BYTES)
            });

        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();
        let mut out = String::new();
        let mut shown_end = offset.saturating_sub(1);
        for (i, l) in lines.iter().enumerate().skip(offset - 1).take(limit) {
            let formatted = format!("{:>4} | {l}\n", i + 1);
            if !out.is_empty() && out.len() + formatted.len() > max_bytes {
                break;
            }
            out.push_str(&formatted);
            shown_end = i + 1;
        }
        if shown_end < total_lines || size > MAX_FILE_INDEX_BYTES {
            let next_offset = shown_end + 1;
            let total_note = if size > MAX_FILE_INDEX_BYTES {
                format!("{total_lines}+ (first {} MiB indexed)", MAX_FILE_INDEX_BYTES / (1024 * 1024))
            } else {
                total_lines.to_string()
            };
            out.push_str(&format!(
                "[Showing lines {offset}-{shown_end} of {total_note} lines. \
                 To continue reading, call read_file(path=\"{rel}\", offset={next_offset}, limit={limit}); \
                 to locate specific content directly, use search_in_file.]\n"
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

// ----- search_in_file ------------------------------------------------------------------------

pub struct SearchInFile {
    ctx: BuiltinContext,
}

tool_def!(SearchInFile, "search_in_file", true,
"Search for a keyword or regular expression inside a single workspace file and return matching line numbers with surrounding context.",
{
    "type": "object",
    "properties": {
        "path": { "type": "string", "description": "relative to the workspace root" },
        "pattern": { "type": "string", "description": "keyword or regular expression to match" },
        "context_lines": { "type": "integer", "minimum": 0, "maximum": 10, "description": "context lines before and after each match (default 2)" },
        "max_matches": { "type": "integer", "minimum": 1, "maximum": 100, "description": "maximum number of matching lines to return (default 30)" }
    },
    "required": ["path", "pattern"]
});

#[async_trait]
impl LocalTool for SearchInFile {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let rel = str_arg(&args, "path")?;
        let pattern = str_arg(&args, "pattern")?;
        if pattern.is_empty() {
            return Err(ToolError::InvalidArguments(
                "`pattern` must not be empty".into(),
            ));
        }
        let context_lines = args
            .get("context_lines")
            .and_then(Value::as_u64)
            .map_or(2usize, |v| (v as usize).min(10));
        let max_matches = args
            .get("max_matches")
            .and_then(Value::as_u64)
            .map_or(30usize, |v| (v as usize).clamp(1, 100));

        let re = regex::Regex::new(pattern)
            .map_err(|e| ToolError::InvalidArguments(format!("invalid regex `{pattern}`: {e}")))?;

        let path = resolve_existing(root(&self.ctx)?, rel)?;
        let mut f = tokio::fs::File::open(&path)
            .await
            .map_err(|e| ToolError::Failed(format!("{rel}: {e}")))?;
        let mut buf = Vec::new();
        (&mut f)
            .take(MAX_FILE_INDEX_BYTES)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        if buf.iter().take(8192).any(|b| *b == 0) {
            return Err(ToolError::Failed(format!("{rel} looks like a binary file")));
        }
        let text = String::from_utf8_lossy(&buf);
        let lines: Vec<&str> = text.lines().collect();

        let mut matches = Vec::new();
        let mut total_matches = 0usize;
        for (idx, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                total_matches += 1;
                if matches.len() < max_matches {
                    matches.push(idx);
                }
            }
        }
        if matches.is_empty() {
            return Ok(ToolOutput::ok(format!(
                "[No matches for `{pattern}` in {rel} ({} lines)]",
                lines.len()
            )));
        }

        let mut out = format!(
            "[Found {total_matches} match(es) in {rel} (showing {})]\n",
            matches.len()
        );
        let mut last_printed: Option<usize> = None;
        for &m in &matches {
            let start = m.saturating_sub(context_lines);
            let end = (m + context_lines + 1).min(lines.len());
            if let Some(prev) = last_printed
                && start > prev + 1
            {
                out.push_str("  ...\n");
            }
            for (i, line) in lines.iter().enumerate().take(end).skip(start) {
                if last_printed.is_some_and(|p| i <= p) {
                    continue;
                }
                let marker = if i == m { '>' } else { ' ' };
                out.push_str(&format!("{marker}{:>4} | {line}\n", i + 1));
                last_printed = Some(i);
            }
        }
        if total_matches > matches.len() {
            out.push_str(&format!(
                "[... {} more match(es) omitted; increase max_matches or narrow pattern]\n",
                total_matches - matches.len()
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
        "path": { "type": "string", "description": "File path relative to the workspace root" },
        "content": { "type": "string", "description": "Full file contents (create/overwrite mode; mutually exclusive with old_string/new_string)" },
        "old_string": { "type": "string", "description": "Exact existing substring to replace (patch mode; requires new_string, mutually exclusive with content)" },
        "new_string": { "type": "string", "description": "Replacement text for old_string (required when old_string is set)" },
        "replace_all": { "type": "boolean", "description": "Replace every occurrence of old_string instead of requiring a unique match (default false)" }
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
        "command": { "type": "string", "description": "Shell command executed via `sh -c` in the workspace root" },
        "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Execution timeout in seconds (default 120, max 600)" }
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
        let grab_raw = |h: tokio::task::JoinHandle<Vec<u8>>| async move {
            match tokio::time::timeout(Duration::from_secs(2), h).await {
                Ok(Ok(b)) => String::from_utf8_lossy(&b).into_owned(),
                _ => String::new(),
            }
        };
        let raw_stdout = grab_raw(read_out).await;
        let raw_stderr = grab_raw(read_err).await;
        let exit_code = status.and_then(|s| s.code());

        let combined_chars = raw_stdout.chars().count() + raw_stderr.chars().count();
        let threshold = max.min(SPILL_THRESHOLD_CHARS);
        let full_combined = if raw_stderr.is_empty() {
            raw_stdout.clone()
        } else if raw_stdout.is_empty() {
            format!("[stderr]\n{raw_stderr}")
        } else {
            format!("[stdout]\n{raw_stdout}\n\n[stderr]\n{raw_stderr}")
        };

        let text = if combined_chars > threshold
            && let Some(spilled) = spill_if_needed(
                &self.ctx,
                "run_command",
                Some(command),
                &full_combined,
                threshold,
                4_000,
                6_000,
            )
            .await
        {
            format!(
                "exit_code: {}, timed_out: {timed_out}\n{spilled}",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".into())
            )
        } else {
            let stdout = trim_middle(&raw_stdout, max);
            let stderr = trim_middle(&raw_stderr, max);
            let body = json!({
                "exit_code": exit_code,
                "timed_out": timed_out,
                "stdout": stdout,
                "stderr": stderr,
            });
            serde_json::to_string_pretty(&body).unwrap_or_default()
        };
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
        .with_tool(Arc::new(SearchInFile { ctx: ctx.clone() }))
        .with_tool(Arc::new(WriteFile { ctx: ctx.clone() }))
        .with_tool(Arc::new(RunCommand { ctx: ctx.clone() }))
}
