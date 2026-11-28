//! Browser side of MCP OAuth: `start` builds the authorization URL (discovery, client
//! registration, PKCE) and remembers the pending request; `callback` exchanges the code and
//! stores the tokens. Token use and refresh live in `oort_mcp::oauth::OAuthTokenProvider`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oort_agent::secret;
use oort_agent::{
    McpAuth, McpServerRecord, McpTransportKind, OAuthClientSource, OAuthStatus, OAuthTokenRecord,
    OAuthTokenStore,
};
use oort_mcp::oauth::{self as flow, Client, OAuthError};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::api::ApiError;

/// How long a started authorization may take.
const PENDING_TTL: Duration = Duration::from_secs(600);

struct Pending {
    server_id: String,
    verifier: String,
    created: Instant,
    /// The token record to store once the code is exchanged (tokens empty until then).
    record: OAuthTokenRecord,
}

/// Manual client settings and requested scopes from `auth: {"type":"oauth", …}`.
#[derive(Debug, Clone, Default)]
pub struct OAuthSettings {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
}

impl OAuthSettings {
    pub fn of(rec: &McpServerRecord) -> Result<Self, ApiError> {
        let McpAuth::OAuth { config } = &rec.auth else {
            return Err(ApiError::BadRequest(
                "this server is not configured for OAuth (set auth type to oauth)".into(),
            ));
        };
        let get = |k: &str| -> Result<Option<String>, ApiError> {
            config
                .get(k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| secret::resolve(v).map_err(|e| ApiError::BadRequest(e.to_string())))
                .transpose()
        };
        Ok(Self {
            client_id: get("client_id")?,
            client_secret: get("client_secret")?,
            scopes: get("scopes")?,
        })
    }
}

/// Public state of a server's OAuth connection (no tokens or secrets).
#[derive(Debug, Clone, Serialize)]
pub struct OAuthView {
    pub status: OAuthStatus,
    pub issuer: String,
    pub scopes: String,
    pub expires_at: Option<i64>,
    pub client_source: OAuthClientSource,
    pub client_id: String,
    pub has_refresh_token: bool,
}

impl From<&OAuthTokenRecord> for OAuthView {
    fn from(t: &OAuthTokenRecord) -> Self {
        Self {
            status: if t.access_token.is_some() {
                t.status
            } else {
                OAuthStatus::AuthRequired
            },
            issuer: t.issuer.clone(),
            scopes: t.scopes.clone(),
            expires_at: t.expires_at,
            client_source: t.client_source,
            client_id: secret::mask(&t.client_id),
            has_refresh_token: t.refresh_token.is_some(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OAuthStart {
    pub authorization_url: String,
    pub state: String,
}

pub struct OAuthFlows {
    store: Arc<dyn OAuthTokenStore>,
    http: reqwest::Client,
    redirect_uri: String,
    pending: Mutex<HashMap<String, Pending>>,
}

fn flow_err(e: OAuthError) -> ApiError {
    match e {
        OAuthError::NotRequired | OAuthError::NoClient | OAuthError::NoPkce => {
            ApiError::BadRequest(e.to_string())
        }
        other => ApiError::BadGateway(other.to_string()),
    }
}

impl OAuthFlows {
    /// `public_url` is where browsers reach this server (the callback is under it).
    pub fn new(store: Arc<dyn OAuthTokenStore>, public_url: &str) -> Self {
        Self {
            store,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            redirect_uri: format!(
                "{}/api/mcp/oauth/callback",
                public_url.trim_end_matches('/')
            ),
            pending: Mutex::default(),
        }
    }

    pub fn store(&self) -> &Arc<dyn OAuthTokenStore> {
        &self.store
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub async fn status(&self, server_id: &str) -> Result<Option<OAuthView>, ApiError> {
        Ok(self
            .store
            .get_oauth_token(server_id)
            .await?
            .as_ref()
            .map(OAuthView::from))
    }

    /// Discovery → client (stored, manual or DCR) → PKCE → authorization URL.
    pub async fn start(&self, rec: &McpServerRecord) -> Result<OAuthStart, ApiError> {
        if rec.transport != McpTransportKind::StreamableHttp {
            return Err(ApiError::BadRequest(
                "OAuth is only for streamable_http servers".into(),
            ));
        }
        let settings = OAuthSettings::of(rec)?;
        let url = rec
            .url
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("server has no url".into()))?;
        let disc = flow::discover(&self.http, url).await.map_err(flow_err)?;
        let scope = settings.scopes.clone().or(disc.scope.clone());

        let existing = self.store.get_oauth_token(&rec.id).await?;
        let (client, source) = match (&settings.client_id, &existing) {
            (Some(id), _) => (
                Client {
                    client_id: id.clone(),
                    client_secret: settings.client_secret.clone(),
                },
                OAuthClientSource::Manual,
            ),
            // Reuse a client registered earlier with the same authorization server.
            (None, Some(t))
                if t.issuer == disc.auth.issuer && t.client_source == OAuthClientSource::Dcr =>
            {
                (
                    Client {
                        client_id: t.client_id.clone(),
                        client_secret: t.client_secret.clone(),
                    },
                    OAuthClientSource::Dcr,
                )
            }
            (None, _) => {
                let endpoint = disc
                    .auth
                    .registration_endpoint
                    .as_deref()
                    .ok_or_else(|| flow_err(OAuthError::NoClient))?;
                let c = flow::register_client(
                    &self.http,
                    endpoint,
                    &self.redirect_uri,
                    scope.as_deref(),
                )
                .await
                .map_err(flow_err)?;
                (c, OAuthClientSource::Dcr)
            }
        };

        let (verifier, challenge) = flow::pkce();
        let state = flow::random_token(32);
        let authorization_url = flow::authorization_url(&flow::AuthorizeParams {
            authorization_endpoint: &disc.auth.authorization_endpoint,
            client_id: &client.client_id,
            redirect_uri: &self.redirect_uri,
            code_challenge: &challenge,
            state: &state,
            scope: scope.as_deref(),
            resource: &disc.resource,
        })
        .map_err(flow_err)?;

        let record = OAuthTokenRecord {
            server_id: rec.id.clone(),
            issuer: disc.auth.issuer.clone(),
            resource: disc.resource.clone(),
            token_endpoint: disc.auth.token_endpoint.clone(),
            registration_endpoint: disc.auth.registration_endpoint.clone(),
            client_id: client.client_id,
            client_secret: client.client_secret,
            client_source: source,
            access_token: None,
            refresh_token: None,
            expires_at: None,
            scopes: scope.unwrap_or_default(),
            status: OAuthStatus::AuthRequired,
            updated_at: flow::now_ms(),
        };
        let mut pending = self.pending.lock().await;
        pending.retain(|_, p| p.created.elapsed() < PENDING_TTL);
        pending.insert(
            state.clone(),
            Pending {
                server_id: rec.id.clone(),
                verifier,
                created: Instant::now(),
                record,
            },
        );
        Ok(OAuthStart {
            authorization_url,
            state,
        })
    }

    /// Validates `state` (exact match, single use, not expired), exchanges `code` and stores the
    /// tokens. Returns the server id.
    pub async fn callback(&self, code: &str, state: &str) -> Result<String, ApiError> {
        let p = self
            .pending
            .lock()
            .await
            .remove(state)
            .filter(|p| p.created.elapsed() < PENDING_TTL)
            .ok_or_else(|| ApiError::BadRequest("unknown or expired authorization state".into()))?;
        let mut rec = p.record;
        let client = Client {
            client_id: rec.client_id.clone(),
            client_secret: rec.client_secret.clone(),
        };
        let tokens = flow::exchange_code(
            &self.http,
            &rec.token_endpoint,
            &client,
            code,
            &p.verifier,
            &self.redirect_uri,
            &rec.resource,
        )
        .await
        .map_err(flow_err)?;
        flow::apply_tokens(&mut rec, tokens, flow::now_ms());
        self.store.put_oauth_token(&rec).await?;
        tracing::info!(server = %p.server_id, scopes = %rec.scopes, "MCP OAuth connected");
        Ok(p.server_id)
    }

    /// Server id of a pending authorization (for error pages).
    pub async fn pending_server(&self, state: &str) -> Option<String> {
        self.pending
            .lock()
            .await
            .get(state)
            .map(|p| p.server_id.clone())
    }

    pub async fn disconnect(&self, server_id: &str) -> Result<(), ApiError> {
        self.store.delete_oauth_token(server_id).await?;
        Ok(())
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Page shown in the authorization popup: notifies the opener and closes itself; without an
/// opener it links back to the server's page.
pub fn callback_page(server_id: Option<&str>, error: Option<&str>) -> String {
    let ok = error.is_none();
    let msg = serde_json::json!({ "type": "oort-oauth", "server_id": server_id, "ok": ok, "error": error });
    let back = format!("/#/mcp/{}", server_id.unwrap_or_default());
    let title = if ok {
        "認証が完了しました"
    } else {
        "認証に失敗しました"
    };
    let detail = error
        .map(html_escape)
        .unwrap_or_else(|| "このウィンドウは自動的に閉じます。".into());
    format!(
        r#"<!doctype html><html lang="ja"><head><meta charset="utf-8"><title>Oort · {title}</title>
<style>body{{font:15px/1.6 system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0;background:#FAF9FC;color:#1D1A26}}
main{{text-align:center;max-width:28rem;padding:2rem}}a{{color:#6247C2}}</style></head>
<body><main><h1 style="font-size:1.2rem">{title}</h1><p>{detail}</p><p><a href="{back}">Oort に戻る</a></p></main>
<script>
const msg = {msg};
if (window.opener) {{ window.opener.postMessage(msg, "*"); if (msg.ok) setTimeout(() => window.close(), 600); }}
else if (msg.ok) {{ location.replace({back_js}); }}
</script></body></html>"#,
        msg = msg,
        back = html_escape(&back),
        back_js = serde_json::to_string(&back).unwrap_or_default(),
    )
}
