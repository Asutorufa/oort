//! The subset of JSON-RPC 2.0 and MCP types the client needs.
//!
//! All dynamic JSON is `serde_json::Value`, whose objects are `BTreeMap`-backed (this workspace
//! never enables `serde_json/preserve_order`), so schemas re-serialize with sorted keys no
//! matter what order the server sent.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Protocol revision requested in `initialize`.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("JSON-RPC error {code}: {message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// A decoded incoming JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    /// A request from the server (sampling, roots, ping, …).
    Request {
        id: Value,
        method: String,
    },
    Notification {
        method: String,
    },
}

impl Incoming {
    pub fn parse(v: &Value) -> Option<Self> {
        let method = v.get("method").and_then(Value::as_str);
        match (v.get("id"), method) {
            (Some(id), Some(m)) => Some(Self::Request {
                id: id.clone(),
                method: m.to_string(),
            }),
            (None, Some(m)) => Some(Self::Notification {
                method: m.to_string(),
            }),
            (Some(id), None) => {
                let id = id.as_u64()?;
                let result = match v.get("error") {
                    Some(e) => Err(serde_json::from_value(e.clone()).unwrap_or(RpcError {
                        code: -32603,
                        message: e.to_string(),
                        data: None,
                    })),
                    None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                Some(Self::Response { id, result })
            }
            (None, None) => None,
        }
    }
}

pub fn request(id: u64, method: &str, params: Option<Value>) -> Value {
    let mut v = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if let Some(p) = params {
        v["params"] = p;
    }
    v
}

pub fn notification(method: &str, params: Option<Value>) -> Value {
    let mut v = json!({ "jsonrpc": "2.0", "method": method });
    if let Some(p) = params {
        v["params"] = p;
    }
    v
}

/// Reply to server-initiated requests: `ping` succeeds, everything else is unsupported
/// (this client advertises no capabilities).
pub fn reply_to_server_request(id: &Value, method: &str) -> Value {
    if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": RpcError::METHOD_NOT_FOUND, "message": format!("unsupported method: {method}") }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    pub server_info: Implementation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

impl McpTool {
    /// Missing `readOnlyHint` is treated as **not** read-only (safe default: sequential).
    pub fn is_read_only(&self) -> bool {
        self.annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContents {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Content block of a tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    Text {
        text: String,
    },
    Image {
        #[serde(rename = "mimeType", default)]
        mime_type: String,
    },
    Audio {
        #[serde(rename = "mimeType", default)]
        mime_type: String,
    },
    Resource {
        resource: ResourceContents,
    },
    ResourceLink {
        uri: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
}

impl CallToolResult {
    /// Flattens the result into the text `ToolResult::content` carries.
    /// Binary content becomes a placeholder; structured content is used when there is no text.
    pub fn to_text(&self) -> String {
        let mut parts: Vec<String> = self
            .content
            .iter()
            .map(|c| match c {
                McpContent::Text { text } => text.clone(),
                McpContent::Image { mime_type } => format!("[image: {mime_type}]"),
                McpContent::Audio { mime_type } => format!("[audio: {mime_type}]"),
                McpContent::Resource { resource } => resource
                    .text
                    .clone()
                    .unwrap_or_else(|| format!("[resource: {}]", resource.uri)),
                McpContent::ResourceLink { uri } => format!("[resource link: {uri}]"),
                McpContent::Unknown => "[unsupported content]".to_string(),
            })
            .collect();
        if parts.is_empty()
            && let Some(sc) = &self.structured_content
        {
            parts.push(sc.to_string());
        }
        parts.join("\n")
    }
}
