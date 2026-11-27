//! Built-in local tools for the console: two read-only file tools, a clock, and one
//! side-effecting tool (so the approval flow can be exercised without MCP).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use oort_agent::{LocalTool, LocalToolExecutor, Tool, ToolContext, ToolError, ToolOutput};
use oort_llm::ToolDefinition;
use serde_json::{Value, json};

pub fn builtin_executor(workspace: &Path) -> LocalToolExecutor {
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    LocalToolExecutor::new()
        .with_tool(Arc::new(GetTime))
        .with_tool(Arc::new(ListDir { root: root.clone() }))
        .with_tool(Arc::new(ReadFile { root: root.clone() }))
        .with_tool(Arc::new(WriteNote {
            path: root.join("oort-notes.md"),
        }))
}

/// Resolves `rel` inside `root`, rejecting escapes (`..`, absolute paths, symlinks out).
fn resolve(root: &Path, rel: &str) -> Result<PathBuf, ToolError> {
    let joined = root.join(rel);
    let canon = joined
        .canonicalize()
        .map_err(|e| ToolError::Failed(format!("{rel}: {e}")))?;
    if !canon.starts_with(root) {
        return Err(ToolError::InvalidArguments(format!(
            "{rel} is outside the workspace"
        )));
    }
    Ok(canon)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing string argument `{key}`")))
}

struct GetTime;

impl Tool for GetTime {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_time",
            "Current time as unix milliseconds",
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

#[async_trait]
impl LocalTool for GetTime {
    async fn run(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Ok(ToolOutput::ok(format!("unix_ms={ms}")))
    }
}

struct ListDir {
    root: PathBuf,
}

impl Tool for ListDir {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list_dir",
            "List a directory inside the workspace",
            json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        )
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

#[async_trait]
impl LocalTool for ListDir {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let dir = resolve(&self.root, str_arg(&args, "path").unwrap_or("."))?;
        let mut rd = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let mut names = Vec::new();
        while let Some(e) = rd
            .next_entry()
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
        {
            let dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            names.push(format!(
                "{}{}",
                e.file_name().to_string_lossy(),
                if dir { "/" } else { "" }
            ));
        }
        names.sort();
        Ok(ToolOutput::ok(names.join("\n")))
    }
}

struct ReadFile {
    root: PathBuf,
}

impl Tool for ReadFile {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read_file",
            "Read a UTF-8 file inside the workspace (first 64 KiB)",
            json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        )
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

#[async_trait]
impl LocalTool for ReadFile {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let path = resolve(&self.root, str_arg(&args, "path")?)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let end = bytes.len().min(64 * 1024);
        Ok(ToolOutput::ok(
            String::from_utf8_lossy(&bytes[..end]).into_owned(),
        ))
    }
}

struct WriteNote {
    path: PathBuf,
}

impl Tool for WriteNote {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write_note",
            "Append a line to oort-notes.md in the workspace (side effect: needs approval)",
            json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
        )
    }
}

#[async_trait]
impl LocalTool for WriteNote {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        use tokio::io::AsyncWriteExt;
        let text = str_arg(&args, "text")?;
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        f.write_all(format!("- {text}\n").as_bytes())
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(ToolOutput::ok(format!(
            "saved note to {}",
            self.path.display()
        )))
    }
}
