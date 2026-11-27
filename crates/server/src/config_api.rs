//! JSON contract of the configuration endpoints (providers, MCP servers, presets).
//!
//! Views never contain secrets: API keys, bearer tokens and sensitive header / env values are
//! masked (`AIza…9x2Q`); `env:NAME` references are shown as is. Inputs use the "keep on empty"
//! rule of [`oort_agent::secret::merge`], so a form can send back what it was shown.

use std::collections::BTreeMap;

use oort_agent::secret::{self, SecretSource};
use oort_agent::{
    ApprovalPolicyKind, McpAuth, McpServerRecord, McpTransportKind, PresetRecord, ProviderKind,
    ProviderRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::ApiError;
use crate::{new_id, now_ms};

fn required(v: Option<String>, field: &str) -> Result<String, ApiError> {
    v.map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest(format!("`{field}` is required")))
}

/// `Some("")` clears an optional text field; `None` keeps it.
fn merge_opt(current: Option<String>, incoming: Option<String>) -> Option<String> {
    match incoming {
        None => current,
        Some(v) => Some(v.trim().to_string()).filter(|v| !v.is_empty()),
    }
}

// ----- providers -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_source: SecretSource,
    pub default_model: String,
    pub extra_headers: BTreeMap<String, String>,
    pub is_enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&ProviderRecord> for ProviderView {
    fn from(p: &ProviderRecord) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            kind: p.kind,
            base_url: p.base_url.clone(),
            api_key: p.api_key.as_deref().map(secret::mask),
            api_key_source: secret::source(p.api_key.as_deref()),
            default_model: p.default_model.clone(),
            extra_headers: secret::mask_map(&p.extra_headers),
            is_enabled: p.is_enabled,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderInput {
    pub name: Option<String>,
    pub kind: Option<ProviderKind>,
    pub base_url: Option<String>,
    /// Empty, missing or the masked value: keep the stored key.
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub extra_headers: Option<BTreeMap<String, String>>,
    pub is_enabled: Option<bool>,
    /// Test only: also send a 1-token chat request.
    #[serde(default)]
    pub ping: bool,
}

impl ProviderInput {
    pub fn create(self) -> Result<ProviderRecord, ApiError> {
        let now = now_ms();
        let rec = ProviderRecord {
            id: new_id(),
            name: required(self.name, "name")?,
            kind: self
                .kind
                .ok_or_else(|| ApiError::BadRequest("`kind` is required".into()))?,
            base_url: merge_opt(None, self.base_url),
            api_key: secret::merge(None, self.api_key.as_deref()),
            default_model: self.default_model.unwrap_or_default().trim().to_string(),
            extra_headers: secret::merge_map(&BTreeMap::new(), self.extra_headers.as_ref()),
            is_enabled: self.is_enabled.unwrap_or(true),
            created_at: now,
            updated_at: now,
        };
        validate_provider(&rec)?;
        Ok(rec)
    }

    /// Merges into `cur` (missing fields keep their value). The kind cannot change: sessions
    /// created with this provider depend on its wire format.
    pub fn apply(self, cur: &ProviderRecord) -> Result<ProviderRecord, ApiError> {
        if let Some(kind) = self.kind
            && kind != cur.kind
        {
            return Err(ApiError::BadRequest(
                "`kind` cannot be changed; create a new provider instead".into(),
            ));
        }
        let rec = ProviderRecord {
            id: cur.id.clone(),
            name: match self.name {
                Some(n) => required(Some(n), "name")?,
                None => cur.name.clone(),
            },
            kind: cur.kind,
            base_url: merge_opt(cur.base_url.clone(), self.base_url),
            api_key: secret::merge(cur.api_key.as_deref(), self.api_key.as_deref()),
            default_model: self
                .default_model
                .map(|m| m.trim().to_string())
                .unwrap_or_else(|| cur.default_model.clone()),
            extra_headers: secret::merge_map(&cur.extra_headers, self.extra_headers.as_ref()),
            is_enabled: self.is_enabled.unwrap_or(cur.is_enabled),
            created_at: cur.created_at,
            updated_at: now_ms().max(cur.updated_at + 1),
        };
        validate_provider(&rec)?;
        Ok(rec)
    }
}

fn validate_provider(p: &ProviderRecord) -> Result<(), ApiError> {
    if p.kind == ProviderKind::OpenaiCompatible && p.base_url.is_none() {
        return Err(ApiError::BadRequest(
            "openai_compatible providers need a base_url".into(),
        ));
    }
    Ok(())
}

// ----- MCP servers ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthView {
    None,
    Bearer {
        token: String,
        token_source: SecretSource,
    },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(flatten)]
        config: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerView {
    pub id: String,
    pub name: String,
    pub transport: McpTransportKind,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub auth: AuthView,
    pub is_enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&McpServerRecord> for McpServerView {
    fn from(m: &McpServerRecord) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            transport: m.transport,
            url: m.url.clone(),
            command: m.command.clone(),
            args: m.args.clone(),
            env: secret::mask_map(&m.env),
            headers: secret::mask_map(&m.headers),
            auth: match &m.auth {
                McpAuth::None => AuthView::None,
                McpAuth::Bearer { token } => AuthView::Bearer {
                    token: secret::mask(token),
                    token_source: secret::source(Some(token)),
                },
                McpAuth::OAuth { config } => AuthView::OAuth {
                    config: config
                        .iter()
                        .map(|(k, v)| {
                            let v = match v {
                                Value::String(s) if secret::is_sensitive_name(k) => {
                                    Value::String(secret::mask(s))
                                }
                                other => other.clone(),
                            };
                            (k.clone(), v)
                        })
                        .collect(),
                },
            },
            is_enabled: m.is_enabled,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthInput {
    None,
    /// Empty / missing / masked token keeps the stored one.
    Bearer {
        #[serde(default)]
        token: Option<String>,
    },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(flatten)]
        config: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpServerInput {
    pub name: Option<String>,
    pub transport: Option<McpTransportKind>,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
    pub headers: Option<BTreeMap<String, String>>,
    pub auth: Option<AuthInput>,
    pub is_enabled: Option<bool>,
}

fn merge_auth(cur: &McpAuth, incoming: Option<AuthInput>) -> Result<McpAuth, ApiError> {
    Ok(match incoming {
        None => cur.clone(),
        Some(AuthInput::None) => McpAuth::None,
        Some(AuthInput::Bearer { token }) => {
            let current = match cur {
                McpAuth::Bearer { token } => Some(token.as_str()),
                _ => None,
            };
            McpAuth::Bearer {
                token: secret::merge(current, token.as_deref())
                    .ok_or_else(|| ApiError::BadRequest("bearer auth needs a token".into()))?,
            }
        }
        Some(AuthInput::OAuth { config }) => McpAuth::OAuth { config },
    })
}

impl McpServerInput {
    pub fn create(self) -> Result<McpServerRecord, ApiError> {
        let now = now_ms();
        let empty = McpServerRecord {
            id: new_id(),
            name: String::new(),
            transport: self
                .transport
                .ok_or_else(|| ApiError::BadRequest("`transport` is required".into()))?,
            url: None,
            command: None,
            args: vec![],
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            auth: McpAuth::None,
            is_enabled: true,
            created_at: now,
            updated_at: now,
        };
        if self.name.is_none() {
            return Err(ApiError::BadRequest("`name` is required".into()));
        }
        let mut rec = self.apply(&empty)?;
        rec.updated_at = now;
        Ok(rec)
    }

    pub fn apply(self, cur: &McpServerRecord) -> Result<McpServerRecord, ApiError> {
        let rec = McpServerRecord {
            id: cur.id.clone(),
            name: match self.name {
                Some(n) => required(Some(n), "name")?,
                None => cur.name.clone(),
            },
            transport: self.transport.unwrap_or(cur.transport),
            url: merge_opt(cur.url.clone(), self.url),
            command: merge_opt(cur.command.clone(), self.command),
            args: self.args.unwrap_or_else(|| cur.args.clone()),
            env: secret::merge_map(&cur.env, self.env.as_ref()),
            headers: secret::merge_map(&cur.headers, self.headers.as_ref()),
            auth: merge_auth(&cur.auth, self.auth)?,
            is_enabled: self.is_enabled.unwrap_or(cur.is_enabled),
            created_at: cur.created_at,
            updated_at: now_ms().max(cur.updated_at + 1),
        };
        validate_mcp(&rec)?;
        Ok(rec)
    }
}

fn validate_mcp(m: &McpServerRecord) -> Result<(), ApiError> {
    let name_ok = m.name.len() <= 32
        && m.name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !m.name.contains("__");
    if !name_ok {
        return Err(ApiError::BadRequest(
            "`name` must be 1-32 characters of [A-Za-z0-9_-] without `__` (it prefixes tool names)"
                .into(),
        ));
    }
    match m.transport {
        McpTransportKind::Stdio if m.command.is_none() => Err(ApiError::BadRequest(
            "stdio servers need a `command`".into(),
        )),
        McpTransportKind::StreamableHttp if m.url.is_none() => Err(ApiError::BadRequest(
            "streamable_http servers need a `url`".into(),
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfoView {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub read_only: bool,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpTestResponse {
    pub ok: bool,
    pub latency_ms: u64,
    pub server_info: Option<ServerInfoView>,
    pub tools: Vec<ToolInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallRequest {
    pub tool_name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallResponse {
    /// The server answered (even with `isError`); false on protocol/transport failures.
    pub ok: bool,
    pub latency_ms: u64,
    pub is_error: bool,
    /// MCP content blocks, verbatim.
    pub content: Value,
    pub structured_content: Option<Value>,
    /// Content flattened as the agent would see it.
    pub text: Option<String>,
    pub error: Option<String>,
}

// ----- presets -------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PresetInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub mcp_server_ids: Option<Vec<String>>,
    pub builtin_tools: Option<bool>,
    pub approval_policy: Option<ApprovalPolicyKind>,
}

impl PresetInput {
    pub fn create(self) -> Result<PresetRecord, ApiError> {
        let now = now_ms();
        let cur = PresetRecord {
            id: new_id(),
            name: String::new(),
            description: String::new(),
            provider_id: required(self.provider_id.clone(), "provider_id")?,
            model: String::new(),
            system_prompt: String::new(),
            mcp_server_ids: vec![],
            builtin_tools: true,
            approval_policy: ApprovalPolicyKind::default(),
            created_at: now,
            updated_at: now,
        };
        if self.name.is_none() {
            return Err(ApiError::BadRequest("`name` is required".into()));
        }
        let mut rec = self.apply(&cur)?;
        rec.updated_at = now;
        Ok(rec)
    }

    pub fn apply(self, cur: &PresetRecord) -> Result<PresetRecord, ApiError> {
        Ok(PresetRecord {
            id: cur.id.clone(),
            name: match self.name {
                Some(n) => required(Some(n), "name")?,
                None => cur.name.clone(),
            },
            description: self.description.unwrap_or_else(|| cur.description.clone()),
            provider_id: match self.provider_id {
                Some(p) => required(Some(p), "provider_id")?,
                None => cur.provider_id.clone(),
            },
            model: self
                .model
                .map(|m| m.trim().to_string())
                .unwrap_or_else(|| cur.model.clone()),
            system_prompt: self
                .system_prompt
                .unwrap_or_else(|| cur.system_prompt.clone()),
            mcp_server_ids: self
                .mcp_server_ids
                .unwrap_or_else(|| cur.mcp_server_ids.clone()),
            builtin_tools: self.builtin_tools.unwrap_or(cur.builtin_tools),
            approval_policy: self.approval_policy.unwrap_or(cur.approval_policy),
            created_at: cur.created_at,
            updated_at: now_ms().max(cur.updated_at + 1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_views_mask_and_updates_keep_secrets() {
        let rec = ProviderInput {
            name: Some("g".into()),
            kind: Some(ProviderKind::Gemini),
            api_key: Some("AIzaSyD-0123456789-9x2Q".into()),
            extra_headers: Some(BTreeMap::from([
                ("x-api-key".into(), "supersecretvalue1234".into()),
                ("x-org".into(), "acme".into()),
            ])),
            ..Default::default()
        }
        .create()
        .unwrap();
        let view = ProviderView::from(&rec);
        assert_eq!(view.api_key.as_deref(), Some("AIza…9x2Q"));
        assert_eq!(view.api_key_source, SecretSource::Inline);
        assert_eq!(view.extra_headers["x-api-key"], "supe…1234");
        assert_eq!(view.extra_headers["x-org"], "acme");
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("0123456789") && !json.contains("supersecret"));

        // Echo the view back (what a form does) with one change: secrets survive.
        let updated = ProviderInput {
            api_key: view.api_key.clone(),
            extra_headers: Some(view.extra_headers.clone()),
            default_model: Some("gemini-2.5-flash".into()),
            ..Default::default()
        }
        .apply(&rec)
        .unwrap();
        assert_eq!(updated.api_key, rec.api_key);
        assert_eq!(updated.extra_headers, rec.extra_headers);
        assert_eq!(updated.default_model, "gemini-2.5-flash");
        assert!(updated.updated_at > rec.updated_at);

        // Empty keeps, env reference replaces and is shown unmasked.
        let kept = ProviderInput {
            api_key: Some(String::new()),
            ..Default::default()
        }
        .apply(&rec)
        .unwrap();
        assert_eq!(kept.api_key, rec.api_key);
        let env = ProviderInput {
            api_key: Some("env:GEMINI_API_KEY".into()),
            ..Default::default()
        }
        .apply(&rec)
        .unwrap();
        let v = ProviderView::from(&env);
        assert_eq!(v.api_key.as_deref(), Some("env:GEMINI_API_KEY"));
        assert_eq!(v.api_key_source, SecretSource::Env);

        let err = ProviderInput {
            kind: Some(ProviderKind::Openai),
            ..Default::default()
        }
        .apply(&rec)
        .unwrap_err();
        assert!(err.to_string().contains("kind"));
    }

    #[test]
    fn mcp_validation_and_token_merge() {
        let rec = McpServerInput {
            name: Some("remote".into()),
            transport: Some(McpTransportKind::StreamableHttp),
            url: Some("http://localhost:9/mcp".into()),
            auth: Some(AuthInput::Bearer {
                token: Some("tok-0123456789abcdef".into()),
            }),
            ..Default::default()
        }
        .create()
        .unwrap();
        let McpServerView { auth, .. } = McpServerView::from(&rec);
        let AuthView::Bearer { token, .. } = auth else {
            panic!()
        };
        assert_eq!(token, "tok-…cdef");
        let kept = McpServerInput {
            auth: Some(AuthInput::Bearer { token: Some(token) }),
            ..Default::default()
        }
        .apply(&rec)
        .unwrap();
        assert_eq!(kept.auth, rec.auth);

        for bad in [
            McpServerInput {
                name: Some("bad name".into()),
                transport: Some(McpTransportKind::Stdio),
                command: Some("x".into()),
                ..Default::default()
            },
            McpServerInput {
                name: Some("a__b".into()),
                transport: Some(McpTransportKind::Stdio),
                command: Some("x".into()),
                ..Default::default()
            },
            McpServerInput {
                name: Some("nocmd".into()),
                transport: Some(McpTransportKind::Stdio),
                ..Default::default()
            },
        ] {
            assert!(bad.create().is_err());
        }
    }
}
