//! OAuth 2.0 for remote MCP servers (MCP Authorization spec).
//!
//! Protocol pieces only (no storage, no HTTP server):
//! - discovery: `401` + `WWW-Authenticate: Bearer resource_metadata="…"` → RFC 9728 protected
//!   resource metadata → RFC 8414 authorization server metadata (with OIDC fallback);
//! - Dynamic Client Registration (RFC 7591);
//! - Authorization Code + PKCE `S256` + `resource` indicator (RFC 8707);
//! - token exchange and refresh;
//! - [`OAuthTokenProvider`]: a [`TokenProvider`] backed by an [`OAuthTokenStore`] that refreshes
//!   before expiry and after a `401`, and marks the server `auth_required` when refresh fails.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oort_agent::{OAuthStatus, OAuthTokenRecord, OAuthTokenStore};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::Url;

use crate::transport::{TokenProvider, TransportError};

/// Refresh this long before `expires_at`.
pub const REFRESH_SKEW_MS: i64 = 60_000;

#[derive(Debug, Clone, thiserror::Error)]
pub enum OAuthError {
    #[error("this server does not require OAuth (it answered without a 401)")]
    NotRequired,
    #[error("discovery failed: {0}")]
    Discovery(String),
    #[error("the authorization server does not support PKCE S256")]
    NoPkce,
    #[error("no client: dynamic registration is unavailable and no client_id is configured")]
    NoClient,
    #[error("client registration failed: {0}")]
    Registration(String),
    #[error("token request failed: {0}")]
    Token(String),
    #[error("http: {0}")]
    Http(String),
}

fn http_err(e: reqwest::Error) -> OAuthError {
    OAuthError::Http(e.to_string())
}

// ----- discovery -----------------------------------------------------------------------------

/// What `WWW-Authenticate: Bearer …` carried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Challenge {
    pub resource_metadata: Option<String>,
    pub scope: Option<String>,
}

/// Parses the `Bearer` challenge parameters (quoted or token values).
pub fn parse_www_authenticate(header: &str) -> Challenge {
    let mut out = Challenge::default();
    let rest = header
        .trim()
        .strip_prefix("Bearer")
        .or_else(|| header.trim().strip_prefix("bearer"))
        .unwrap_or(header);
    let mut s = rest.trim();
    while !s.is_empty() {
        let Some(eq) = s.find('=') else { break };
        let key = s[..eq]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        s = s[eq + 1..].trim_start();
        let value;
        if let Some(q) = s.strip_prefix('"') {
            let mut v = String::new();
            let mut chars = q.char_indices();
            let mut end = q.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            v.push(n);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => v.push(c),
                }
            }
            value = v;
            s = q[end.min(q.len())..]
                .trim_start()
                .trim_start_matches(',')
                .trim_start();
        } else {
            let end = s.find(',').unwrap_or(s.len());
            value = s[..end].trim().to_string();
            s = s[end..].trim_start_matches(',').trim_start();
        }
        match key.as_str() {
            "resource_metadata" => out.resource_metadata = Some(value),
            "scope" => out.scope = Some(value),
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

/// Everything needed to start an authorization.
#[derive(Debug, Clone)]
pub struct Discovery {
    /// Canonical resource URI to bind tokens to (RFC 8707).
    pub resource: String,
    pub auth: AuthServerMetadata,
    /// Scope to request: the challenge's, else the resource's supported scopes.
    pub scope: Option<String>,
}

/// `https://h/p/` → `https://h/p` (no fragment, no trailing slash): the canonical MCP resource.
pub fn canonical_resource(url: &Url) -> String {
    let mut u = url.clone();
    u.set_fragment(None);
    u.set_query(None);
    u.as_str().trim_end_matches('/').to_string()
}

/// `/.well-known/<name>` URLs to try for `base`: path-inserted first, then root.
fn well_known(base: &Url, name: &str) -> Vec<String> {
    let path = base.path().trim_end_matches('/');
    let origin = base.origin().ascii_serialization();
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{name}{path}"));
    }
    out.push(format!("{origin}/.well-known/{name}"));
    out
}

async fn get_json<T: serde::de::DeserializeOwned>(http: &reqwest::Client, url: &str) -> Option<T> {
    let resp = http
        .get(url)
        .header("accept", "application/json")
        .header("MCP-Protocol-Version", crate::protocol::PROTOCOL_VERSION)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Runs the MCP authorization discovery for `mcp_url`.
pub async fn discover(http: &reqwest::Client, mcp_url: &str) -> Result<Discovery, OAuthError> {
    let url = Url::parse(mcp_url).map_err(|e| OAuthError::Discovery(format!("url: {e}")))?;
    // An unauthenticated request tells us where the metadata is.
    let probe = http
        .post(url.clone())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(
            &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": crate::protocol::PROTOCOL_VERSION, "capabilities": {},
            "clientInfo": {"name": "oort", "version": env!("CARGO_PKG_VERSION")}}}),
        )
        .send()
        .await
        .map_err(http_err)?;
    if probe.status().is_success() {
        return Err(OAuthError::NotRequired);
    }
    let challenge = probe
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .map(parse_www_authenticate)
        .unwrap_or_default();

    let mut candidates: Vec<String> = challenge.resource_metadata.iter().cloned().collect();
    candidates.extend(well_known(&url, "oauth-protected-resource"));
    let mut prm = None;
    for c in &candidates {
        if let Some(m) = get_json::<ProtectedResourceMetadata>(http, c).await {
            prm = Some(m);
            break;
        }
    }
    let prm = prm.ok_or_else(|| {
        OAuthError::Discovery("no protected resource metadata (RFC 9728) found".into())
    })?;
    let issuer = prm
        .authorization_servers
        .first()
        .ok_or_else(|| OAuthError::Discovery("metadata lists no authorization_servers".into()))?;
    let issuer_url =
        Url::parse(issuer).map_err(|e| OAuthError::Discovery(format!("issuer: {e}")))?;
    let mut as_urls = well_known(&issuer_url, "oauth-authorization-server");
    as_urls.extend(well_known(&issuer_url, "openid-configuration"));
    // OIDC also defines `{issuer}/.well-known/openid-configuration` (path appended).
    as_urls.push(format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    ));
    let mut auth = None;
    for u in &as_urls {
        if let Some(m) = get_json::<AuthServerMetadata>(http, u).await {
            auth = Some(m);
            break;
        }
    }
    let auth = auth.ok_or_else(|| {
        OAuthError::Discovery(format!("no authorization server metadata for {issuer}"))
    })?;
    if !auth
        .code_challenge_methods_supported
        .iter()
        .any(|m| m == "S256")
    {
        return Err(OAuthError::NoPkce);
    }
    let resource = prm.resource.unwrap_or_else(|| canonical_resource(&url));
    let scope = challenge
        .scope
        .or_else(|| (!prm.scopes_supported.is_empty()).then(|| prm.scopes_supported.join(" ")));
    Ok(Discovery {
        resource,
        auth,
        scope,
    })
}

// ----- PKCE / state --------------------------------------------------------------------------

/// URL-safe random string from `bytes` bytes of OS randomness.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS randomness is available");
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `(code_verifier, code_challenge)`; the verifier is 43+ chars as RFC 7636 requires.
pub fn pkce() -> (String, String) {
    let verifier = random_token(48);
    let challenge = s256(&verifier);
    (verifier, challenge)
}

// ----- client registration -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Deserialize)]
struct RegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

/// RFC 7591 registration of a public client using the authorization code flow.
pub async fn register_client(
    http: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
    scope: Option<&str>,
) -> Result<Client, OAuthError> {
    let mut body = json!({
        "client_name": "Oort",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    if let Some(s) = scope {
        body["scope"] = Value::String(s.to_string());
    }
    let resp = http
        .post(registration_endpoint)
        .json(&body)
        .send()
        .await
        .map_err(http_err)?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::Registration(format!("HTTP {status}: {text}")));
    }
    let r: RegistrationResponse = resp
        .json()
        .await
        .map_err(|e| OAuthError::Registration(e.to_string()))?;
    Ok(Client {
        client_id: r.client_id,
        client_secret: r.client_secret,
    })
}

// ----- authorization & tokens ----------------------------------------------------------------

pub struct AuthorizeParams<'a> {
    pub authorization_endpoint: &'a str,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub code_challenge: &'a str,
    pub state: &'a str,
    pub scope: Option<&'a str>,
    pub resource: &'a str,
}

/// The URL the user opens (values are percent-encoded in full, never truncated).
pub fn authorization_url(p: &AuthorizeParams<'_>) -> Result<String, OAuthError> {
    let mut url = Url::parse(p.authorization_endpoint)
        .map_err(|e| OAuthError::Discovery(format!("authorization_endpoint: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", p.client_id)
            .append_pair("redirect_uri", p.redirect_uri)
            .append_pair("code_challenge", p.code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", p.state)
            .append_pair("resource", p.resource);
        if let Some(s) = p.scope.filter(|s| !s.is_empty()) {
            q.append_pair("scope", s);
        }
    }
    Ok(url.into())
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

async fn token_request(
    http: &reqwest::Client,
    token_endpoint: &str,
    client: &Client,
    mut form: Vec<(&str, String)>,
) -> Result<TokenResponse, OAuthError> {
    form.push(("client_id", client.client_id.clone()));
    let mut req = http
        .post(token_endpoint)
        .header("accept", "application/json");
    if let Some(secret) = &client.client_secret {
        // client_secret_post: the most widely accepted method for MCP authorization servers.
        form.push(("client_secret", secret.clone()));
    }
    req = req.form(&form);
    let resp = req.send().await.map_err(http_err)?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::Token(format!("HTTP {status}: {text}")));
    }
    resp.json()
        .await
        .map_err(|e| OAuthError::Token(format!("invalid token response: {e}")))
}

#[allow(clippy::too_many_arguments)]
pub async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    client: &Client,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    resource: &str,
) -> Result<TokenResponse, OAuthError> {
    token_request(
        http,
        token_endpoint,
        client,
        vec![
            ("grant_type", "authorization_code".into()),
            ("code", code.into()),
            ("code_verifier", code_verifier.into()),
            ("redirect_uri", redirect_uri.into()),
            ("resource", resource.into()),
        ],
    )
    .await
}

pub async fn refresh(
    http: &reqwest::Client,
    token_endpoint: &str,
    client: &Client,
    refresh_token: &str,
    resource: &str,
) -> Result<TokenResponse, OAuthError> {
    token_request(
        http,
        token_endpoint,
        client,
        vec![
            ("grant_type", "refresh_token".into()),
            ("refresh_token", refresh_token.into()),
            ("resource", resource.into()),
        ],
    )
    .await
}

/// Applies a token response to the stored record (keeps the old refresh token if none is sent).
pub fn apply_tokens(rec: &mut OAuthTokenRecord, t: TokenResponse, now_ms: i64) {
    rec.access_token = Some(t.access_token);
    if t.refresh_token.is_some() {
        rec.refresh_token = t.refresh_token;
    }
    rec.expires_at = t.expires_in.map(|s| now_ms + s * 1000);
    if let Some(scope) = t.scope {
        rec.scopes = scope;
    }
    rec.status = OAuthStatus::Connected;
    rec.updated_at = now_ms;
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

// ----- token provider ------------------------------------------------------------------------

/// Bearer tokens for one MCP server from the token store, refreshed when needed.
pub struct OAuthTokenProvider {
    server_id: String,
    store: Arc<dyn OAuthTokenStore>,
    http: reqwest::Client,
    /// Serializes refreshes; `true` forces the next `token()` to refresh (after a 401).
    stale: Mutex<bool>,
}

impl OAuthTokenProvider {
    pub fn new(server_id: impl Into<String>, store: Arc<dyn OAuthTokenStore>) -> Self {
        Self {
            server_id: server_id.into(),
            store,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            stale: Mutex::new(false),
        }
    }

    async fn mark_auth_required(&self, mut rec: OAuthTokenRecord, why: &str) -> TransportError {
        tracing::warn!(server = %self.server_id, reason = %why, "MCP OAuth: re-authorization required");
        rec.status = OAuthStatus::AuthRequired;
        rec.updated_at = now_ms();
        let _ = self.store.put_oauth_token(&rec).await;
        TransportError::Unauthorized
    }
}

#[async_trait]
impl TokenProvider for OAuthTokenProvider {
    async fn token(&self) -> Result<String, TransportError> {
        let mut stale = self.stale.lock().await;
        let rec = self
            .store
            .get_oauth_token(&self.server_id)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?
            .ok_or(TransportError::Unauthorized)?;
        if rec.status == OAuthStatus::AuthRequired {
            return Err(TransportError::Unauthorized);
        }
        let expiring = rec
            .expires_at
            .is_some_and(|at| at - REFRESH_SKEW_MS <= now_ms());
        if let Some(token) = &rec.access_token
            && !expiring
            && !*stale
        {
            return Ok(token.clone());
        }
        let Some(refresh_token) = rec.refresh_token.clone() else {
            return Err(self
                .mark_auth_required(rec, "token expired and no refresh token")
                .await);
        };
        let client = Client {
            client_id: rec.client_id.clone(),
            client_secret: rec.client_secret.clone(),
        };
        match refresh(
            &self.http,
            &rec.token_endpoint,
            &client,
            &refresh_token,
            &rec.resource,
        )
        .await
        {
            Ok(t) => {
                let mut rec = rec;
                apply_tokens(&mut rec, t, now_ms());
                self.store
                    .put_oauth_token(&rec)
                    .await
                    .map_err(|e| TransportError::Io(e.to_string()))?;
                *stale = false;
                tracing::info!(server = %self.server_id, "MCP OAuth token refreshed");
                Ok(rec.access_token.unwrap_or_default())
            }
            Err(e) => Err(self.mark_auth_required(rec, &e.to_string()).await),
        }
    }

    /// Called after a `401`: the next `token()` refreshes even if the token looks valid.
    async fn invalidate(&self) {
        *self.stale.lock().await = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_challenges() {
        let c = parse_www_authenticate(
            r#"Bearer realm="mcp", resource_metadata="https://h/.well-known/oauth-protected-resource/mcp", scope="read write""#,
        );
        assert_eq!(
            c.resource_metadata.as_deref(),
            Some("https://h/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(c.scope.as_deref(), Some("read write"));
        let c = parse_www_authenticate(
            r#"Bearer error=invalid_token, resource_metadata=https://h/m, scope="a \"q\" b""#,
        );
        assert_eq!(c.resource_metadata.as_deref(), Some("https://h/m"));
        assert_eq!(c.scope.as_deref(), Some("a \"q\" b"));
        assert_eq!(parse_www_authenticate("Basic"), Challenge::default());
    }

    #[test]
    fn pkce_is_s256_of_the_verifier() {
        // RFC 7636 appendix B.
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let (v, c) = pkce();
        assert!(v.len() >= 43 && v.len() <= 128);
        assert_eq!(s256(&v), c);
        assert_ne!(random_token(32), random_token(32));
    }

    #[test]
    fn urls() {
        let u = Url::parse("https://h.example/api/mcp/?x=1#f").unwrap();
        assert_eq!(canonical_resource(&u), "https://h.example/api/mcp");
        assert_eq!(
            well_known(&u, "oauth-protected-resource"),
            [
                "https://h.example/.well-known/oauth-protected-resource/api/mcp",
                "https://h.example/.well-known/oauth-protected-resource"
            ]
        );
        let long_state = "s".repeat(5000);
        let long_scope = (0..800)
            .map(|i| format!("sc{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let url = authorization_url(&AuthorizeParams {
            authorization_endpoint: "https://as/authorize",
            client_id: "c",
            redirect_uri: "http://127.0.0.1:3000/api/mcp/oauth/callback",
            code_challenge: "ch",
            state: &long_state,
            scope: Some(&long_scope),
            resource: "https://h/mcp",
        })
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let get = |k: &str| {
            parsed
                .query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(get("state").unwrap(), long_state, "state kept in full");
        assert_eq!(get("scope").unwrap(), long_scope, "scope kept in full");
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(get("resource").as_deref(), Some("https://h/mcp"));
    }
}
