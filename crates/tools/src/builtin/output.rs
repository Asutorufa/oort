//! Spill-to-storage overflow cache and `read_tool_output` pagination/search tool.
//!
//! When `web_fetch` or `run_command` produces output longer than [`SPILL_THRESHOLD_CHARS`],
//! the full text (capped at [`MAX_BLOB_BYTES`]) is stored in SQLite (`tool_output_blobs`)
//! under an `output_id`, and the tool returns a preview with explicit instructions for
//! `read_tool_output`.

use std::sync::Arc;

use async_trait::async_trait;
use oort_agent::{
    LocalTool, LocalToolExecutor, ToolContext, ToolError, ToolOutput, ToolOutputBlob,
};
use serde_json::Value;

use super::{BuiltinContext, missing, opt_str, str_arg, tool_def};

/// Threshold (in chars) above which `web_fetch` / `run_command` spill full output to storage.
pub const SPILL_THRESHOLD_CHARS: usize = 12_000;

/// Hard upper bound on a single cached blob (2 MiB) to protect disk space.
pub const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Default and maximum character window returned by `read_tool_output`.
const DEFAULT_LIMIT_CHARS: usize = 8_000;
const MAX_LIMIT_CHARS: usize = 16_000;

/// Truncates `s` to at most `max_bytes` on a valid UTF-8 char boundary.
fn cap_utf8_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// If `content` exceeds `threshold_chars` and a session-scoped blob store is available, persists
/// the full output and returns a preview + continuation prompt. Returns `None` if no spill is
/// needed or if no blob store is configured.
pub async fn spill_if_needed(
    ctx: &BuiltinContext,
    tool_name: &str,
    source_hint: Option<&str>,
    content: &str,
    threshold_chars: usize,
    head_chars: usize,
    tail_chars: usize,
) -> Option<String> {
    let total_chars = content.chars().count();
    if total_chars <= threshold_chars {
        return None;
    }
    let store = ctx.output_blobs.as_ref()?;
    let session_id = ctx.session_id.as_deref()?;

    let capped = cap_utf8_bytes(content, MAX_BLOB_BYTES);
    let stored_chars = capped.chars().count();
    let stored_lines = capped.lines().count();
    let short = uuid::Uuid::now_v7().simple().to_string();
    let output_id = format!("out_{}", &short[..12]);

    let blob = ToolOutputBlob {
        id: output_id.clone(),
        session_id: session_id.to_string(),
        tool_name: tool_name.to_string(),
        source_hint: source_hint.map(str::to_string),
        content: capped.to_string(),
        total_chars: stored_chars,
        total_lines: stored_lines,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    if let Err(e) = store.save_tool_output_blob(&blob).await {
        tracing::warn!(error = %e, tool = %tool_name, "failed to spill tool output blob");
        return None;
    }

    let preview = if tail_chars == 0 {
        capped.chars().take(head_chars).collect::<String>()
    } else {
        let head: String = capped.chars().take(head_chars).collect();
        let tail: String = capped
            .chars()
            .skip(stored_chars.saturating_sub(tail_chars))
            .collect();
        let omitted = stored_chars.saturating_sub(head_chars + tail_chars);
        format!("{head}\n…[{omitted} characters omitted]…\n{tail}")
    };

    let next_offset = head_chars.min(stored_chars);
    let cap_note = if content.len() > MAX_BLOB_BYTES {
        " (cached up to 2 MiB limit)"
    } else {
        ""
    };
    Some(format!(
        "{preview}\n\n[Output truncated: {stored_chars} chars / {stored_lines} lines total{cap_note}. \
         Full output cached as output_id=\"{output_id}\". \
         Call read_tool_output(output_id=\"{output_id}\", offset={next_offset}, limit=8000) to paginate, \
         or pass query=\"keyword\" to search the full output.]"
    ))
}

// ----- read_tool_output ----------------------------------------------------------------------

pub struct ReadToolOutput {
    ctx: BuiltinContext,
}

tool_def!(ReadToolOutput, "read_tool_output", true,
"Read or search a cached long tool output by output_id (returned when web_fetch or run_command output is truncated). Supports character pagination (offset/limit) or keyword search (query).",
{
    "type": "object",
    "properties": {
        "output_id": { "type": "string", "description": "Cached output identifier (e.g. out_...)" },
        "offset": { "type": "integer", "minimum": 0, "description": "Character offset to start reading from (default 0)" },
        "limit": { "type": "integer", "minimum": 100, "maximum": 16000, "description": "Maximum characters to return (default 8000, max 16000)" },
        "query": { "type": "string", "description": "Optional keyword/substring to search within the full cached output" }
    },
    "required": ["output_id"]
});

#[async_trait]
impl LocalTool for ReadToolOutput {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let output_id = str_arg(&args, "output_id")?;
        let store = self
            .ctx
            .output_blobs
            .as_ref()
            .ok_or_else(|| missing("tool output cache"))?;
        let session_id = self
            .ctx
            .session_id
            .as_deref()
            .ok_or_else(|| missing("session context"))?;

        let blob = store
            .get_tool_output_blob(output_id)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .ok_or_else(|| ToolError::Failed(format!("unknown output_id: {output_id}")))?;

        let allowed = store
            .can_access_blob(session_id, &blob.session_id)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        if !allowed {
            return Err(ToolError::Failed(format!(
                "access denied: output_id `{output_id}` does not belong to this session"
            )));
        }

        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_LIMIT_CHARS, |v| (v as usize).clamp(100, MAX_LIMIT_CHARS));

        if let Some(query) = opt_str(&args, "query") {
            return Ok(ToolOutput::ok(search_blob(&blob, query, limit)));
        }

        let offset = args
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        if offset >= blob.total_chars {
            return Ok(ToolOutput::ok(format!(
                "[End of output `{output_id}`: offset {offset} >= total {} chars]",
                blob.total_chars
            )));
        }
        let slice: String = blob.content.chars().skip(offset).take(limit).collect();
        let shown_chars = slice.chars().count();
        let end = offset + shown_chars;
        let mut out = slice;
        if end < blob.total_chars {
            out.push_str(&format!(
                "\n\n[Showing chars {offset}-{end} of {} total. Call read_tool_output(output_id=\"{output_id}\", offset={end}, limit={limit}) to continue.]",
                blob.total_chars
            ));
        } else {
            out.push_str(&format!(
                "\n\n[End of output `{output_id}`: showing chars {offset}-{end} of {} total.]",
                blob.total_chars
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

fn search_blob(blob: &ToolOutputBlob, query: &str, max_chars: usize) -> String {
    let needle = query.to_lowercase();
    let lines: Vec<&str> = blob.content.lines().collect();
    let mut matched_indices = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.to_lowercase().contains(&needle) {
            matched_indices.push(idx);
        }
    }
    if matched_indices.is_empty() {
        return format!(
            "[No matches for {:?} in output_id=\"{}\" ({} chars / {} lines)]",
            query, blob.id, blob.total_chars, blob.total_lines
        );
    }

    let mut out = format!(
        "[Found {} matching line(s) for {:?} in output_id=\"{}\" ({})]\n",
        matched_indices.len(),
        query,
        blob.id,
        blob.source_hint.as_deref().unwrap_or(&blob.tool_name)
    );
    let mut last_printed: Option<usize> = None;
    let context = 2usize;

    for &m in &matched_indices {
        let start = m.saturating_sub(context);
        let end = (m + context + 1).min(lines.len());
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
            let formatted = format!("{marker}{:>5} | {line}\n", i + 1);
            if out.chars().count() + formatted.chars().count() > max_chars {
                out.push_str(&format!(
                    "\n[Search output reached {max_chars} char limit; refine query or use offset/limit.]\n"
                ));
                return out;
            }
            out.push_str(&formatted);
            last_printed = Some(i);
        }
    }
    out
}

pub(super) fn add(ex: LocalToolExecutor, ctx: &BuiltinContext) -> LocalToolExecutor {
    ex.with_tool(Arc::new(ReadToolOutput { ctx: ctx.clone() }))
}
