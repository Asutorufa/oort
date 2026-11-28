//! JSON contract of the configuration endpoints (providers, MCP servers, presets).
//!
//! Views never contain secrets: API keys, bearer tokens and sensitive header / env values are
//! masked (`AIza…9x2Q`); `env:NAME` references are shown as is. Inputs use the "keep on empty"
//! rule of [`oort_agent::secret::merge`], so a form can send back what it was shown.

use std::collections::BTreeMap;

use oort_agent::secret::{self, SecretSource};
use oort_agent::{
    ApprovalPolicyKind, AutoRetry, McpAuth, McpServerRecord, McpTransportKind, PresetRecord,
    ProviderDefaults, ProviderKind, ProviderRecord, ReasoningEffort, ServerToolConfig, Vendor,
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
    pub vendor: Vendor,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_key_source: SecretSource,
    pub default_model: String,
    pub extra_headers: BTreeMap<String, String>,
    pub default_server_tools: ServerToolConfig,
    pub extra_body: Option<Value>,
    pub is_enabled: bool,
    pub auto_retry: AutoRetry,
    pub defaults: ProviderDefaults,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&ProviderRecord> for ProviderView {
    fn from(p: &ProviderRecord) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            kind: p.kind,
            vendor: p.vendor,
            base_url: p.base_url.clone(),
            api_key: p.api_key.as_deref().map(secret::mask),
            api_key_source: secret::source(p.api_key.as_deref()),
            default_model: p.default_model.clone(),
            extra_headers: secret::mask_map(&p.extra_headers),
            default_server_tools: p.default_server_tools.clone(),
            extra_body: p.extra_body.clone(),
            is_enabled: p.is_enabled,
            auto_retry: p.auto_retry,
            defaults: p.defaults,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderInput {
    pub name: Option<String>,
    pub kind: Option<ProviderKind>,
    pub vendor: Option<Vendor>,
    pub base_url: Option<String>,
    /// Empty, missing or the masked value: keep the stored key.
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub extra_headers: Option<BTreeMap<String, String>>,
    pub default_server_tools: Option<ServerToolConfig>,
    pub extra_body: Option<Value>,
    pub is_enabled: Option<bool>,
    pub auto_retry: Option<AutoRetry>,
    /// Replaces all generation defaults when present.
    pub defaults: Option<ProviderDefaults>,
    /// Test only: also send a 1-token chat request.
    #[serde(default)]
    pub ping: bool,
}

fn default_vendor_for_kind(kind: ProviderKind) -> Vendor {
    match kind {
        ProviderKind::Openai => Vendor::Openai,
        ProviderKind::Anthropic => Vendor::Anthropic,
        ProviderKind::Gemini => Vendor::Gemini,
        ProviderKind::OpenaiCompatible | ProviderKind::Mock => Vendor::Custom,
    }
}

fn normalize_extra_body(v: Option<Value>) -> Result<Option<Value>, ApiError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) if m.is_empty() => Ok(None),
        Some(v @ Value::Object(_)) => Ok(Some(v)),
        Some(_) => Err(ApiError::BadRequest(
            "`extra_body` must be a JSON object".into(),
        )),
    }
}

impl ProviderInput {
    pub fn create(self) -> Result<ProviderRecord, ApiError> {
        let now = now_ms();
        let kind = self
            .kind
            .ok_or_else(|| ApiError::BadRequest("`kind` is required".into()))?;
        let rec = ProviderRecord {
            id: new_id(),
            name: required(self.name, "name")?,
            kind,
            vendor: self.vendor.unwrap_or_else(|| default_vendor_for_kind(kind)),
            base_url: merge_opt(None, self.base_url),
            api_key: secret::merge(None, self.api_key.as_deref()),
            default_model: self.default_model.unwrap_or_default().trim().to_string(),
            extra_headers: secret::merge_map(&BTreeMap::new(), self.extra_headers.as_ref()),
            default_server_tools: self.default_server_tools.unwrap_or_default(),
            extra_body: normalize_extra_body(self.extra_body)?,
            is_enabled: self.is_enabled.unwrap_or(true),
            auto_retry: self.auto_retry.unwrap_or_default(),
            defaults: self.defaults.unwrap_or_default(),
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
        let extra_body = match self.extra_body {
            Some(v) => normalize_extra_body(Some(v))?,
            None => cur.extra_body.clone(),
        };
        let rec = ProviderRecord {
            id: cur.id.clone(),
            name: match self.name {
                Some(n) => required(Some(n), "name")?,
                None => cur.name.clone(),
            },
            kind: cur.kind,
            vendor: self.vendor.unwrap_or(cur.vendor),
            base_url: merge_opt(cur.base_url.clone(), self.base_url),
            api_key: secret::merge(cur.api_key.as_deref(), self.api_key.as_deref()),
            default_model: self
                .default_model
                .map(|m| m.trim().to_string())
                .unwrap_or_else(|| cur.default_model.clone()),
            extra_headers: secret::merge_map(&cur.extra_headers, self.extra_headers.as_ref()),
            default_server_tools: self
                .default_server_tools
                .unwrap_or_else(|| cur.default_server_tools.clone()),
            extra_body,
            is_enabled: self.is_enabled.unwrap_or(cur.is_enabled),
            auto_retry: self.auto_retry.unwrap_or(cur.auto_retry),
            defaults: self.defaults.unwrap_or(cur.defaults),
            created_at: cur.created_at,
            updated_at: now_ms().max(cur.updated_at + 1),
        };
        validate_provider(&rec)?;
        Ok(rec)
    }
}

fn validate_provider(p: &ProviderRecord) -> Result<(), ApiError> {
    let d = &p.defaults;
    if d.temperature.is_some_and(|t| !(0.0..=2.0).contains(&t)) {
        return Err(ApiError::BadRequest(
            "`defaults.temperature` must be between 0 and 2".into(),
        ));
    }
    if d.max_output_tokens == Some(0) || d.context_window.is_some_and(|c| c < 1024) {
        return Err(ApiError::BadRequest(
            "`defaults.max_output_tokens` must be positive and `defaults.context_window` at least 1024"
                .into(),
        ));
    }
    if !(5..=1800).contains(&d.timeout_secs) {
        return Err(ApiError::BadRequest(
            "`defaults.timeout_secs` must be between 5 and 1800".into(),
        ));
    }
    if !(1..=10).contains(&p.auto_retry.max_attempts) {
        return Err(ApiError::BadRequest(
            "`auto_retry.max_attempts` must be between 1 and 10".into(),
        ));
    }
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
    /// OAuth connection state (set by the handlers for `oauth` servers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<crate::oauth::OAuthView>,
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
            oauth: None,
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
        Some(AuthInput::OAuth { config }) => {
            // Sensitive settings (client_secret) arrive masked: keep the stored value.
            let current = match cur {
                McpAuth::OAuth { config } => config.clone(),
                _ => BTreeMap::new(),
            };
            let config = config
                .into_iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) if secret::is_sensitive_name(&k) => {
                        let cur = current.get(&k).and_then(Value::as_str);
                        secret::merge(cur, Some(&s)).map(|m| (k, Value::String(m)))
                    }
                    other => Some((k, other)),
                })
                .collect();
            McpAuth::OAuth { config }
        }
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
    pub skill_ids: Option<Vec<String>>,
    pub builtin_groups: Option<Vec<String>>,
    /// Empty string clears it.
    pub workspace_root: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub enabled_server_tools: Option<ServerToolConfig>,
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
            skill_ids: vec![],
            builtin_groups: vec![],
            workspace_root: None,
            reasoning_effort: ReasoningEffort::Default,
            enabled_server_tools: None,
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
        if let Some(g) = &self.builtin_groups {
            crate::registry::parse_groups(g)?;
        }
        Ok(PresetRecord {
            reasoning_effort: self.reasoning_effort.unwrap_or(cur.reasoning_effort),
            enabled_server_tools: self
                .enabled_server_tools
                .or_else(|| cur.enabled_server_tools.clone()),
            builtin_groups: self
                .builtin_groups
                .clone()
                .unwrap_or_else(|| cur.builtin_groups.clone()),
            workspace_root: match &self.workspace_root {
                Some(r) => Some(r.trim().to_string()).filter(|r| !r.is_empty()),
                None => cur.workspace_root.clone(),
            },
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
            skill_ids: self.skill_ids.unwrap_or_else(|| cur.skill_ids.clone()),
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

// ----- skills --------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SkillView {
    #[serde(flatten)]
    pub skill: oort_skills::Skill,
    pub editable: bool,
}

impl From<oort_skills::Skill> for SkillView {
    fn from(skill: oort_skills::Skill) -> Self {
        Self {
            editable: skill.is_editable(),
            skill,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub user_invocable: Option<bool>,
    /// Empty string clears it.
    pub argument_hint: Option<String>,
    pub execution_mode: Option<oort_agent::SkillExecutionMode>,
    /// Empty string clears it.
    pub subagent_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillsReload {
    pub file_skills: usize,
    pub warnings: Vec<String>,
    pub dirs: Vec<String>,
}

impl SkillInput {
    pub fn create(self) -> Result<oort_agent::SkillRecord, ApiError> {
        let now = now_ms();
        let cur = oort_agent::SkillRecord {
            id: new_id(),
            name: String::new(),
            description: String::new(),
            body: String::new(),
            allowed_tools: vec![],
            tags: vec![],
            invocation: Default::default(),
            created_at: now,
            updated_at: now,
        };
        if self.name.is_none() || self.description.is_none() {
            return Err(ApiError::BadRequest(
                "`name` and `description` are required".into(),
            ));
        }
        let mut rec = self.apply(&cur)?;
        rec.updated_at = now;
        Ok(rec)
    }

    pub fn apply(self, cur: &oort_agent::SkillRecord) -> Result<oort_agent::SkillRecord, ApiError> {
        let rec = oort_agent::SkillRecord {
            id: cur.id.clone(),
            name: self
                .name
                .map(|n| n.trim().to_string())
                .unwrap_or_else(|| cur.name.clone()),
            description: self
                .description
                .map(|d| d.trim().to_string())
                .unwrap_or_else(|| cur.description.clone()),
            body: self.body.unwrap_or_else(|| cur.body.clone()),
            allowed_tools: self
                .allowed_tools
                .unwrap_or_else(|| cur.allowed_tools.clone()),
            tags: self.tags.unwrap_or_else(|| cur.tags.clone()),
            invocation: oort_agent::SkillInvocation {
                user_invocable: self.user_invocable.unwrap_or(cur.invocation.user_invocable),
                argument_hint: match self.argument_hint {
                    Some(h) => Some(h.trim().to_string()).filter(|h| !h.is_empty()),
                    None => cur.invocation.argument_hint.clone(),
                },
                execution_mode: self.execution_mode.unwrap_or(cur.invocation.execution_mode),
                subagent_profile: match self.subagent_profile {
                    Some(p) => Some(p.trim().to_string()).filter(|p| !p.is_empty()),
                    None => cur.invocation.subagent_profile.clone(),
                },
            },
            created_at: cur.created_at,
            updated_at: now_ms().max(cur.updated_at + 1),
        };
        if !oort_skills::valid_name(&rec.name) {
            return Err(ApiError::BadRequest(
                "`name` must be 1-64 characters of [a-z0-9-] (it is also the /command)".into(),
            ));
        }
        if rec.description.is_empty() {
            return Err(ApiError::BadRequest(
                "`description` must not be empty".into(),
            ));
        }
        Ok(rec)
    }
}
