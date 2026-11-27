//! Shared MCP connections, one per server configuration version.
//!
//! Sessions and the tool playground reuse a connection while the server's record is unchanged
//! (`updated_at`). Updating or deleting a record evicts it: new sessions connect afresh, while
//! sessions already holding the old connection keep it until they end (stdio processes are
//! killed when the last holder drops it).

use std::collections::BTreeMap;
use std::sync::Arc;

use oort_agent::McpServerRecord;
use oort_mcp::{ClientOptions, McpError, McpToolExecutor, connect_record};
use tokio::sync::Mutex;

struct Conn {
    version: i64,
    executor: Arc<McpToolExecutor>,
}

pub struct McpPool {
    options: ClientOptions,
    conns: Mutex<BTreeMap<String, Conn>>,
}

impl McpPool {
    pub fn new(options: ClientOptions) -> Self {
        Self {
            options,
            conns: Mutex::default(),
        }
    }

    pub fn options(&self) -> &ClientOptions {
        &self.options
    }

    /// The connection for `rec` (its tool catalog is the one listed at connect time).
    pub async fn executor(&self, rec: &McpServerRecord) -> Result<Arc<McpToolExecutor>, McpError> {
        let mut conns = self.conns.lock().await;
        if let Some(c) = conns.get(&rec.id)
            && c.version == rec.updated_at
        {
            return Ok(c.executor.clone());
        }
        let client = Arc::new(connect_record(rec, self.options.clone()).await?);
        let executor = Arc::new(McpToolExecutor::new(client).await?);
        conns.insert(
            rec.id.clone(),
            Conn {
                version: rec.updated_at,
                executor: executor.clone(),
            },
        );
        Ok(executor)
    }

    /// Drops the cached connection (after an edit, a delete, a fresh discovery or a transport
    /// failure).
    pub async fn evict(&self, id: &str) {
        self.conns.lock().await.remove(id);
    }

    pub async fn close_all(&self) {
        let conns = std::mem::take(&mut *self.conns.lock().await);
        for c in conns.values() {
            c.executor.client().close().await;
        }
    }
}
