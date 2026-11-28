//! MCP OAuth 2.0 end to end against an in-process authorization server + protected MCP server:
//! discovery from the 401, DCR, PKCE S256, resource indicators, callback, refresh (proactive and
//! after a 401), auth_required on refresh failure, manual-client fallback, disconnect.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use common::*;
use oort_agent::OAuthTokenStore;
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

#[derive(Default)]
struct AuthState {
    base: String,
    dcr: bool,
    long_scope: String,
    registrations: Vec<Value>,
    /// code → (challenge, redirect_uri, resource, client_id)
    codes: HashMap<String, (String, String, String, String)>,
    valid_access: Option<String>,
    refresh_tokens: HashSet<String>,
    refresh_ok: bool,
    expires_in: i64,
    issued: u32,
    token_forms: Vec<HashMap<String, String>>,
}

type Shared = Arc<Mutex<AuthState>>;

fn issue(st: &mut AuthState) -> Value {
    st.issued += 1;
    let at = format!("at-{}", st.issued);
    let rt = format!("rt-{}", st.issued);
    st.valid_access = Some(at.clone());
    st.refresh_tokens.insert(rt.clone());
    json!({ "access_token": at, "token_type": "Bearer", "expires_in": st.expires_in, "refresh_token": rt, "scope": st.long_scope })
}

async fn mcp(State(st): State<Shared>, headers: HeaderMap, body: String) -> Response {
    let st_ref = st.lock().unwrap();
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ok = st_ref
        .valid_access
        .as_ref()
        .is_some_and(|t| auth == format!("Bearer {t}"));
    if !ok {
        let challenge = format!(
            r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource/mcp", scope="{}""#,
            st_ref.base, st_ref.long_scope
        );
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", challenge)],
            "",
        )
            .into_response();
    }
    let token = auth.trim_start_matches("Bearer ").to_string();
    drop(st_ref);
    let req: Value = serde_json::from_str(&body).unwrap();
    let Some(id) = req.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match req["method"].as_str().unwrap() {
        "initialize" => {
            json!({ "protocolVersion": req["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "secure-mcp", "version": "1"} })
        }
        "tools/list" => {
            json!({ "tools": [{ "name": "whoami", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true} }] })
        }
        "tools/call" => {
            json!({ "content": [{ "type": "text", "text": format!("token {token}") }] })
        }
        m => panic!("unexpected {m}"),
    };
    axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

async fn start_auth_server(dcr: bool, long_scope: &str) -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let st: Shared = Arc::new(Mutex::new(AuthState {
        base: base.clone(),
        dcr,
        long_scope: long_scope.into(),
        refresh_ok: true,
        expires_in: 3600,
        ..Default::default()
    }));
    let app = axum::Router::new()
        .route("/mcp", post(mcp).delete(|| async { StatusCode::OK }))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(|State(st): State<Shared>| async move {
                let b = st.lock().unwrap().base.clone();
                axum::Json(json!({ "resource": format!("{b}/mcp"), "authorization_servers": [format!("{b}/as")] }))
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server/as",
            get(|State(st): State<Shared>| async move {
                let s = st.lock().unwrap();
                let b = &s.base;
                let mut m = json!({
                    "issuer": format!("{b}/as"),
                    "authorization_endpoint": format!("{b}/as/authorize"),
                    "token_endpoint": format!("{b}/as/token"),
                    "code_challenge_methods_supported": ["S256"],
                });
                if s.dcr {
                    m["registration_endpoint"] = json!(format!("{b}/as/register"));
                }
                axum::Json(m)
            }),
        )
        .route(
            "/as/register",
            post(|State(st): State<Shared>, axum::Json(body): axum::Json<Value>| async move {
                st.lock().unwrap().registrations.push(body);
                axum::Json(json!({ "client_id": "dyn-client" }))
            }),
        )
        .route(
            "/as/token",
            post(|State(st): State<Shared>, Form(f): Form<HashMap<String, String>>| async move {
                let mut s = st.lock().unwrap();
                s.token_forms.push(f.clone());
                let bad = |m: &str| (StatusCode::BAD_REQUEST, axum::Json(json!({ "error": "invalid_grant", "error_description": m }))).into_response();
                match f.get("grant_type").map(String::as_str) {
                    Some("authorization_code") => {
                        let Some((challenge, redirect, resource, client)) = s.codes.remove(&f["code"]) else {
                            return bad("unknown code");
                        };
                        if oort_mcp::oauth::s256(&f["code_verifier"]) != challenge {
                            return bad("PKCE verification failed");
                        }
                        if f["redirect_uri"] != redirect || f["resource"] != resource || f["client_id"] != client {
                            return bad("parameter mismatch");
                        }
                        axum::Json(issue(&mut s)).into_response()
                    }
                    Some("refresh_token") => {
                        if !s.refresh_ok || !s.refresh_tokens.remove(&f["refresh_token"]) {
                            return bad("refresh rejected");
                        }
                        axum::Json(issue(&mut s)).into_response()
                    }
                    _ => bad("unsupported grant"),
                }
            }),
        )
        .with_state(st.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, st)
}

fn query(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn callback(s: &Server, params: &[(&str, &str)]) -> String {
    let url =
        url::Url::parse_with_params(&format!("{}/mcp/oauth/callback", s.base), params).unwrap();
    reqwest::get(url).await.unwrap().text().await.unwrap()
}

async fn call_whoami(s: &Server, id: &str) -> Value {
    s.ok(
        "POST",
        &format!("/mcp-servers/{id}/tools/call"),
        Some(json!({ "tool_name": "whoami" })),
    )
    .await
}

#[tokio::test]
async fn oauth_discovery_dcr_pkce_callback_refresh_and_disconnect() {
    let long_scope = (0..600)
        .map(|i| format!("mcp:scope-{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(long_scope.len() > 4096);
    let (as_base, auth) = start_auth_server(true, &long_scope).await;
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store.clone(), &temp_dir("oauth-ws"), vec![]).await;

    let m = s
        .ok("POST", "/mcp-servers", Some(json!({ "name": "secure", "transport": "streamable_http", "url": format!("{as_base}/mcp"), "auth": { "type": "oauth" } })))
        .await;
    let mid = m["id"].as_str().unwrap().to_string();
    assert!(m.get("oauth").is_none(), "not connected yet");
    let t = s
        .ok("POST", &format!("/mcp-servers/{mid}/test"), None)
        .await;
    assert_eq!(t["ok"], false, "{t}");

    // Start: discovery from the 401, DCR, PKCE.
    let start = s
        .ok("POST", &format!("/mcp-servers/{mid}/oauth/start"), None)
        .await;
    let q = query(start["authorization_url"].as_str().unwrap());
    assert!(
        start["authorization_url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{as_base}/as/authorize?"))
    );
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], "dyn-client");
    assert_eq!(
        q["redirect_uri"],
        format!("{}/api/mcp/oauth/callback", s.origin)
    );
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["resource"], format!("{as_base}/mcp"));
    assert_eq!(q["scope"], long_scope, "long scope kept in full");
    assert_eq!(q["state"], start["state"].as_str().unwrap());
    {
        let a = auth.lock().unwrap();
        assert_eq!(a.registrations.len(), 1);
        assert_eq!(
            a.registrations[0]["redirect_uris"][0],
            q["redirect_uri"].as_str()
        );
        assert_eq!(a.registrations[0]["token_endpoint_auth_method"], "none");
    }

    // The user "authorizes": the AS issues a code bound to the challenge.
    auth.lock().unwrap().codes.insert(
        "code-1".into(),
        (
            q["code_challenge"].clone(),
            q["redirect_uri"].clone(),
            q["resource"].clone(),
            "dyn-client".into(),
        ),
    );
    // A forged (long) state is rejected without touching the token endpoint.
    let page = callback(&s, &[("code", "code-1"), ("state", &"x".repeat(5000))]).await;
    assert!(
        page.contains("\"ok\":false") && page.contains("unknown or expired"),
        "{page}"
    );
    let page = callback(&s, &[("code", "code-1"), ("state", &q["state"])]).await;
    assert!(
        page.contains("\"type\":\"oort-oauth\"") && page.contains("\"ok\":true"),
        "{page}"
    );
    // States are single use.
    let page = callback(&s, &[("code", "code-1"), ("state", &q["state"])]).await;
    assert!(page.contains("\"ok\":false"));
    {
        let a = auth.lock().unwrap();
        let f = &a.token_forms[0];
        assert_eq!(f["grant_type"], "authorization_code");
        assert_eq!(f["resource"], format!("{as_base}/mcp"));
        assert!(!f.contains_key("client_secret"), "public client");
    }

    // Status is visible, secrets are not.
    let view = s.get(&format!("/mcp-servers/{mid}")).await;
    assert_eq!(view["oauth"]["status"], "connected");
    assert_eq!(view["oauth"]["scopes"], long_scope.as_str());
    assert_eq!(view["oauth"]["client_source"], "dcr");
    assert_eq!(view["oauth"]["has_refresh_token"], true);
    let listed = s.get("/mcp-servers").await.to_string();
    assert!(!listed.contains("at-1") && !listed.contains("rt-1"));
    let stored = store.get_oauth_token(&mid).await.unwrap().unwrap();
    assert_eq!(stored.scopes, long_scope, "stored in full");

    // Connected: discovery and the playground use the token.
    let t = s
        .ok("POST", &format!("/mcp-servers/{mid}/test"), None)
        .await;
    assert_eq!(t["ok"], true, "{t}");
    assert_eq!(t["tools"][0]["name"], "whoami");
    assert_eq!(call_whoami(&s, &mid).await["text"], "token at-1");

    // The server revokes the token: a 401 triggers a refresh and one retry.
    auth.lock().unwrap().valid_access = Some("revoked".into());
    assert_eq!(call_whoami(&s, &mid).await["text"], "token at-2");

    // Close to expiry: refreshed before use.
    let mut rec = store.get_oauth_token(&mid).await.unwrap().unwrap();
    rec.expires_at = Some(oort_mcp::oauth::now_ms() - 1);
    store.put_oauth_token(&rec).await.unwrap();
    assert_eq!(call_whoami(&s, &mid).await["text"], "token at-3");
    let refreshes = auth
        .lock()
        .unwrap()
        .token_forms
        .iter()
        .filter(|f| f["grant_type"] == "refresh_token")
        .count();
    assert_eq!(refreshes, 2);

    // Refresh fails → auth_required, and calls report it instead of hanging.
    {
        let mut a = auth.lock().unwrap();
        a.refresh_ok = false;
        a.valid_access = Some("revoked-again".into());
    }
    let r = call_whoami(&s, &mid).await;
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(
        s.get(&format!("/mcp-servers/{mid}")).await["oauth"]["status"],
        "auth_required"
    );

    // Re-authorize reuses the registered client (no second registration).
    let start = s
        .ok("POST", &format!("/mcp-servers/{mid}/oauth/start"), None)
        .await;
    assert_eq!(
        query(start["authorization_url"].as_str().unwrap())["client_id"],
        "dyn-client"
    );
    assert_eq!(auth.lock().unwrap().registrations.len(), 1);

    // Disconnect forgets the tokens.
    s.ok(
        "POST",
        &format!("/mcp-servers/{mid}/oauth/disconnect"),
        None,
    )
    .await;
    assert!(
        s.get(&format!("/mcp-servers/{mid}"))
            .await
            .get("oauth")
            .is_none()
    );
    assert!(store.get_oauth_token(&mid).await.unwrap().is_none());
    s.stop().await;
}

#[tokio::test]
async fn oauth_falls_back_to_a_manual_client_without_dcr() {
    let (as_base, auth) = start_auth_server(false, "mcp:read").await;
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &temp_dir("oauth-ws2"), vec![]).await;

    // No DCR and no client configured: a clear error.
    let bare = s
        .ok("POST", "/mcp-servers", Some(json!({ "name": "nodcr", "transport": "streamable_http", "url": format!("{as_base}/mcp"), "auth": { "type": "oauth" } })))
        .await;
    let r = s
        .send(
            "POST",
            &format!("/mcp-servers/{}/oauth/start", bare["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("no client"));

    let m = s
        .ok(
            "POST",
            "/mcp-servers",
            Some(json!({ "name": "manual", "transport": "streamable_http", "url": format!("{as_base}/mcp"),
                "auth": { "type": "oauth", "client_id": "manual-id", "client_secret": "s3cr3t-value-1234", "scopes": "mcp:read mcp:write" } })),
        )
        .await;
    assert_eq!(m["auth"]["client_secret"], "s3cr…1234", "masked");
    let mid = m["id"].as_str().unwrap().to_string();
    // Echoing the masked secret back keeps it.
    s.ok(
        "PUT",
        &format!("/mcp-servers/{mid}"),
        Some(json!({ "auth": m["auth"] })),
    )
    .await;

    let start = s
        .ok("POST", &format!("/mcp-servers/{mid}/oauth/start"), None)
        .await;
    let q = query(start["authorization_url"].as_str().unwrap());
    assert_eq!(q["client_id"], "manual-id");
    assert_eq!(q["scope"], "mcp:read mcp:write", "configured scopes win");
    auth.lock().unwrap().codes.insert(
        "c".into(),
        (
            q["code_challenge"].clone(),
            q["redirect_uri"].clone(),
            q["resource"].clone(),
            "manual-id".into(),
        ),
    );
    let page = callback(&s, &[("code", "c"), ("state", &q["state"])]).await;
    assert!(page.contains("\"ok\":true"), "{page}");
    assert_eq!(
        auth.lock().unwrap().token_forms[0]["client_secret"],
        "s3cr3t-value-1234"
    );
    assert_eq!(
        s.get(&format!("/mcp-servers/{mid}")).await["oauth"]["client_source"],
        "manual"
    );
    assert_eq!(call_whoami(&s, &mid).await["text"], "token at-1");
    s.stop().await;
}
