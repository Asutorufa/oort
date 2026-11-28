//! Builtin tools: capabilities tied to oort's own state (configuration, memories, the current
//! session's plan and questions) plus a few general-purpose hands. Domain tools stay in MCP.
//!
//! Tools come in groups that a session enables at creation; the resulting catalog is frozen
//! with the session like any other (see `oort_agent::FrozenToolset`).
//!
//! | group | tools |
//! |---|---|
//! | `self_config` | `save_skill`, `configure_mcp_server` |
//! | `core_ux` | `update_plan`, `ask_user` |
//! | `memory` | `memory_save`, `memory_search` |
//! | `utilities` | `current_time`, `web_fetch` |
//! | `workspace` | `read_file`, `write_file`, `run_command` (needs a `workspace_root`) |
//!
//! Approval semantics live in the application's hook: [`ALWAYS_ASK`] tools need a human even
//! under an auto-approve policy, and [`ASK_USER`] waits for an answer instead of running.

pub mod builtin;

pub use builtin::{ALWAYS_ASK, ASK_USER, BuiltinContext, Group, PlanSink, executor, preview};
