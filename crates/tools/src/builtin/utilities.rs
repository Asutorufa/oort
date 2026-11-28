//! `current_time` (the system prompt never carries the time: it would break the prefix cache)
//! and `web_fetch` (HTTP(S) → readable text, with SSRF protection).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oort_agent::{LocalTool, LocalToolExecutor, ToolContext, ToolError, ToolOutput};
use serde_json::{Value, json};

use super::{BuiltinContext, html, str_arg, tool_def};

pub struct CurrentTime;

tool_def!(CurrentTime, "current_time", true,
    "Current date and time (UTC and the server's local time zone). The conversation does not include the time otherwise.",
    { "type": "object", "properties": {} });

/// IANA name of the local zone when it can be found (`TZ`, `/etc/localtime`).
fn zone_name() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.is_empty()
    {
        return Some(tz.trim_start_matches(':').to_string());
    }
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let s = target.to_string_lossy();
    s.split("zoneinfo/").nth(1).map(str::to_string)
}

#[async_trait]
impl LocalTool for CurrentTime {
    async fn run(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let utc = chrono::Utc::now();
        let local = chrono::Local::now();
        Ok(ToolOutput::ok(
            json!({
                "utc": utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "local": local.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                "utc_offset": local.format("%:z").to_string(),
                "timezone": zone_name(),
                "weekday": local.format("%A").to_string(),
                "unix_ms": utc.timestamp_millis(),
            })
            .to_string(),
        ))
    }
}

/// `web_fetch` limits.
#[derive(Debug, Clone)]
pub struct WebFetchOptions {
    pub timeout: Duration,
    pub max_chars: usize,
    pub max_bytes: usize,
    /// Allow loopback / private / link-local targets (tests, trusted setups). Off by default.
    pub allow_private: bool,
}

impl Default for WebFetchOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            max_chars: 30_000,
            max_bytes: 3 * 1024 * 1024,
            allow_private: false,
        }
    }
}

/// Addresses a fetch may not reach (SSRF): loopback, private, link-local, CGNAT, ULA, …
pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || o[0] >= 240
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
        }
    }
}

pub struct WebFetch {
    opts: WebFetchOptions,
}

tool_def!(WebFetch, "web_fetch", true,
"Fetch an http(s) URL and return its readable text (HTML converted to Markdown). Long pages are trimmed.",
{
    "type": "object",
    "properties": {
        "url": { "type": "string" },
        "max_chars": { "type": "integer", "minimum": 500, "maximum": 100000 }
    },
    "required": ["url"]
});

/// Resolves and vets `url`'s host; returns an address to pin the connection to (so a second
/// DNS answer cannot redirect it somewhere private).
async fn vet(url: &url::Url, allow_private: bool) -> Result<SocketAddr, ToolError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ToolError::InvalidArguments(
            "only http and https URLs are allowed".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ToolError::InvalidArguments("URL has no host".into()))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
        .await
        .map_err(|e| ToolError::Failed(format!("cannot resolve {host}: {e}")))?
        .collect();
    let first = *addrs
        .first()
        .ok_or_else(|| ToolError::Failed(format!("{host} has no address")))?;
    if !allow_private && let Some(bad) = addrs.iter().find(|a| is_blocked(a.ip())) {
        return Err(ToolError::Failed(format!(
            "refusing to fetch {host}: it resolves to a local or private address ({})",
            bad.ip()
        )));
    }
    Ok(first)
}

impl WebFetch {
    async fn fetch(&self, start: &str, max_chars: usize) -> Result<String, ToolError> {
        let mut url =
            url::Url::parse(start).map_err(|e| ToolError::InvalidArguments(format!("url: {e}")))?;
        for _hop in 0..6 {
            let addr = vet(&url, self.opts.allow_private).await?;
            let host = url.host_str().unwrap_or_default().to_string();
            let client = reqwest::Client::builder()
                .timeout(self.opts.timeout)
                .redirect(reqwest::redirect::Policy::none())
                .resolve(&host, addr)
                .user_agent("oort-web-fetch/0.1")
                .build()
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            let resp = client
                .get(url.clone())
                .header(
                    "accept",
                    "text/html, text/plain, application/json;q=0.9, */*;q=0.5",
                )
                .send()
                .await
                .map_err(|e| ToolError::Failed(format!("fetch failed: {e}")))?;
            let status = resp.status();
            if status.is_redirection() {
                let loc = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| ToolError::Failed(format!("HTTP {status} without Location")))?;
                url = url
                    .join(loc)
                    .map_err(|e| ToolError::Failed(format!("bad redirect: {e}")))?;
                continue;
            }
            if !status.is_success() {
                return Err(ToolError::Failed(format!("HTTP {status} for {url}")));
            }
            let ctype = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            let textual = ctype.is_empty()
                || ctype.starts_with("text/")
                || ctype.contains("json")
                || ctype.contains("xml");
            if !textual {
                return Err(ToolError::Failed(format!("not a text page ({ctype})")));
            }
            let mut body = Vec::new();
            let mut resp = resp;
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?
            {
                body.extend_from_slice(&chunk);
                if body.len() >= self.opts.max_bytes {
                    break;
                }
            }
            let raw = String::from_utf8_lossy(&body);
            let text = if ctype.contains("html")
                || (ctype.is_empty() && raw.trim_start().starts_with('<'))
            {
                html::to_markdown(&raw, Some(&url))
            } else {
                raw.to_string()
            };
            let total = text.chars().count();
            let mut out: String = text.chars().take(max_chars).collect();
            if total > max_chars {
                out.push_str(&format!(
                    "\n\n[trimmed: showing {max_chars} of {total} characters]"
                ));
            }
            return Ok(format!("Source: {url}\n\n{out}"));
        }
        Err(ToolError::Failed("too many redirects".into()))
    }
}

#[async_trait]
impl LocalTool for WebFetch {
    async fn run(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let url = str_arg(&args, "url")?;
        let max_chars = args
            .get("max_chars")
            .and_then(Value::as_u64)
            .map_or(self.opts.max_chars, |m| (m as usize).clamp(500, 100_000));
        tokio::select! {
            _ = ctx.cancel.cancelled() => Err(ToolError::Cancelled),
            r = self.fetch(url, max_chars) => r.map(ToolOutput::ok),
        }
    }
}

pub(super) fn add(ex: LocalToolExecutor, ctx: &BuiltinContext) -> LocalToolExecutor {
    ex.with_tool(Arc::new(CurrentTime))
        .with_tool(Arc::new(WebFetch {
            opts: ctx.web.clone(),
        }))
}
