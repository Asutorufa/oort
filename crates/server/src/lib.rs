//! Debug-console backend: an axum server that assembles the agent engine, SQLite session store
//! and tool executors (built-in + MCP), runs sessions in the background, and streams their
//! events over SSE.

pub mod api;
pub mod config;
pub mod factory;
pub mod manager;
pub mod mock_model;
pub mod routes;
pub mod tools;

pub use config::{ServerConfig, SessionConfig};
pub use factory::AgentFactory;
pub use manager::SessionManager;
pub use routes::router;
