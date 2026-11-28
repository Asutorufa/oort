//! Workbench backend: an axum server that assembles the agent engine, the SQLite session and
//! configuration stores, LLM providers and tool executors (built-in + MCP), runs sessions in
//! the background, and streams their events over SSE.

pub mod analytics_api;
pub mod api;
pub mod approval;
pub mod commands;
pub mod config;
pub mod config_api;
pub mod hub;
pub mod manager;
pub mod metering;
pub mod mock_model;
pub mod oauth;
pub mod pool;
pub mod registry;
pub mod routes;
pub mod session_tools;
pub mod subagent;
pub mod title;
pub mod tools;

pub use config::{ServerConfig, SessionConfig};
pub use manager::SessionManager;
pub use registry::Registry;
pub use routes::router;

/// Time-ordered unique id (UUID v7).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
