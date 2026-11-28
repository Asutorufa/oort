//! `memory_save` / `memory_search`: cross-session memory, searched on demand (never injected
//! into prompts automatically, which would break the prefix cache).

use std::sync::Arc;

use async_trait::async_trait;
use oort_agent::{
    LocalTool, LocalToolExecutor, MemoryQuery, MemoryRecord, MemoryStore, ToolContext, ToolError,
    ToolOutput,
};
use serde_json::{Value, json};

use super::{BuiltinContext, missing, now_ms, opt_str, str_arg, str_list, tool_def};

pub struct MemorySave {
    store: Option<Arc<dyn MemoryStore>>,
}

tool_def!(MemorySave, "memory_save", false,
"Remember something for future conversations: user preferences, environment conventions, project facts. Pass `id` to update an existing memory.",
{
    "type": "object",
    "properties": {
        "id": { "type": "string", "description": "Existing memory ID to update (omit to create a new memory)" },
        "title": { "type": "string", "description": "Short summary title for the memory" },
        "content": { "type": "string", "description": "Detailed fact, preference, or convention to remember" },
        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional lowercase keywords for filtering" },
        "scope": { "type": "string", "description": "`global` (default) or a project identifier" }
    },
    "required": ["title", "content"]
});

#[async_trait]
impl LocalTool for MemorySave {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let store = self.store.as_ref().ok_or_else(|| missing("memory"))?;
        let now = now_ms();
        let fail = |e: oort_agent::StoreError| ToolError::Failed(e.to_string());
        let mut rec = MemoryRecord {
            id: uuid::Uuid::now_v7().to_string(),
            title: str_arg(&args, "title")?.trim().to_string(),
            content: str_arg(&args, "content")?.to_string(),
            tags: str_list(&args, "tags"),
            scope: opt_str(&args, "scope").unwrap_or("global").to_string(),
            created_at: now,
            updated_at: now,
        };
        if let Some(id) = opt_str(&args, "id") {
            let cur = store.get_memory(id).await.map_err(fail)?;
            rec.id = cur.id;
            rec.created_at = cur.created_at;
            store.update_memory(&rec).await.map_err(fail)?;
            return Ok(ToolOutput::ok(format!(
                "Updated memory {} ({}).",
                rec.id, rec.title
            )));
        }
        store.create_memory(&rec).await.map_err(fail)?;
        Ok(ToolOutput::ok(format!(
            "Saved memory {} ({}).",
            rec.id, rec.title
        )))
    }
}

pub struct MemorySearch {
    store: Option<Arc<dyn MemoryStore>>,
}

tool_def!(MemorySearch, "memory_search", true,
"Search saved memories (full text, any language; filter by tags or scope). Check here before asking the user about preferences or conventions.",
{
    "type": "object",
    "properties": {
        "query": { "type": "string", "description": "Search keywords (matches title and content)" },
        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags filter (must match all specified tags)" },
        "scope": { "type": "string", "description": "Optional scope filter (e.g. `global` or a project identifier)" },
        "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum number of results to return (default 10, max 50)" }
    },
    "required": ["query"]
});

#[async_trait]
impl LocalTool for MemorySearch {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let store = self.store.as_ref().ok_or_else(|| missing("memory"))?;
        let q = MemoryQuery {
            text: args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            tags: str_list(&args, "tags"),
            scope: opt_str(&args, "scope").map(str::to_string),
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|l| l.min(50) as u32),
        };
        let hits = store
            .search_memories(&q)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        if hits.is_empty() {
            return Ok(ToolOutput::ok("No matching memories."));
        }
        let out: Vec<Value> = hits
            .iter()
            .map(|h| json!({ "id": h.memory.id, "title": h.memory.title, "content": h.memory.content, "tags": h.memory.tags, "scope": h.memory.scope }))
            .collect();
        Ok(ToolOutput::ok(
            serde_json::to_string_pretty(&out).unwrap_or_default(),
        ))
    }
}

pub(super) fn add(ex: LocalToolExecutor, ctx: &BuiltinContext) -> LocalToolExecutor {
    ex.with_tool(Arc::new(MemorySave {
        store: ctx.memories.clone(),
    }))
    .with_tool(Arc::new(MemorySearch {
        store: ctx.memories.clone(),
    }))
}
