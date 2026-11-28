use serde_json::Value;
use thiserror::Error;

/// Unified error type returned by all LLM clients in this crate.
#[derive(Debug, Error)]
pub enum Error {
    /// Transport-level HTTP error from `reqwest`.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization or deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// URL construction or parsing error.
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// Error returned by an upstream LLM API (either non-2xx HTTP response or in-stream error event).
    #[error("{}", format_api_error(*.status, message))]
    Api {
        status: Option<u16>,
        message: String,
        body: Option<String>,
    },

    /// Error while reading or decoding a streaming response.
    #[error("Stream error: {0}")]
    Stream(String),

    /// Client or request configuration error (e.g. missing model or invalid header).
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

fn format_api_error(status: Option<u16>, message: &str) -> String {
    match status {
        Some(code) => format!("API error (HTTP {code}): {message}"),
        None => format!("API error: {message}"),
    }
}

impl Error {
    /// Constructs an `Error::Api` from an HTTP status code and raw response body,
    /// extracting the provider's error message when the body is JSON.
    pub fn from_http_response(status: reqwest::StatusCode, body: String) -> Self {
        let extracted = extract_error_message(&body).unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                status
                    .canonical_reason()
                    .unwrap_or("unknown error")
                    .to_string()
            } else {
                truncate_chars(trimmed, 2_000)
            }
        });

        Self::Api {
            status: Some(status.as_u16()),
            message: extracted,
            body: if body.is_empty() { None } else { Some(body) },
        }
    }

    /// Constructs an `Error::Api` without an HTTP status code (e.g. SSE error event).
    pub fn api(message: impl Into<String>) -> Self {
        Self::Api {
            status: None,
            message: message.into(),
            body: None,
        }
    }

    /// Returns the HTTP status code if this is an HTTP API error.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => *status,
            Self::Http(err) => err.status().map(|s| s.as_u16()),
            _ => None,
        }
    }

    /// Returns `true` if this error is typically safe to retry (rate limit, 5xx, or timeout).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Api {
                status: Some(code), ..
            } => matches!(code, 408 | 429 | 500..=599),
            Self::Http(err) => err.is_timeout() || err.is_connect(),
            _ => false,
        }
    }
}

/// Attempts to extract a human-readable message from standard OpenAI, Anthropic, or Gemini JSON error payloads.
pub(crate) fn extract_error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    extract_error_message_from_value(&value)
}

pub(crate) fn extract_error_message_from_value(value: &Value) -> Option<String> {
    // Handles:
    // - OpenAI / Gemini: { "error": { "message": "...", "type": "..." } }
    // - Anthropic: { "type": "error", "error": { "type": "...", "message": "..." } }
    // - OpenAI Responses: { "response": { "error": { "message": "..." } } }
    // - Gemini array error: [ { "error": { "message": "..." } } ]
    if let Some(first) = value.as_array().and_then(|arr| arr.first()) {
        return extract_error_message_from_value(first);
    }

    let err_obj = value
        .pointer("/response/error")
        .or_else(|| value.get("error"));

    if let Some(obj) = err_obj {
        if let Some(msg) = obj.as_str()
            && !msg.trim().is_empty()
        {
            return Some(msg.trim().to_string());
        }
        let msg = obj
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let err_type = obj
            .get("type")
            .or_else(|| obj.get("status"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        return match (err_type, msg) {
            (Some(t), Some(m)) => Some(format!("{t}: {m}")),
            (None, Some(m)) => Some(m.to_string()),
            (Some(t), None) => Some(t.to_string()),
            (None, None) => None,
        };
    }

    value
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let shortened: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_openai_error() {
        let body = r#"{"error":{"message":"Invalid API key","type":"invalid_request_error"}}"#;
        let err = Error::from_http_response(reqwest::StatusCode::UNAUTHORIZED, body.to_string());
        assert_eq!(err.status_code(), Some(401));
        assert!(!err.is_retryable());
        assert!(
            err.to_string()
                .contains("invalid_request_error: Invalid API key")
        );
    }

    #[test]
    fn test_extract_gemini_error() {
        let body =
            r#"{"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED"}}"#;
        let err =
            Error::from_http_response(reqwest::StatusCode::TOO_MANY_REQUESTS, body.to_string());
        assert_eq!(err.status_code(), Some(429));
        assert!(err.is_retryable());
        assert!(
            err.to_string()
                .contains("RESOURCE_EXHAUSTED: Quota exceeded")
        );
    }

    #[test]
    fn test_extract_plain_text_and_empty_error() {
        let err = Error::from_http_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "backend crashed".to_string(),
        );
        assert!(err.is_retryable());
        assert!(err.to_string().contains("backend crashed"));

        let empty = Error::from_http_response(reqwest::StatusCode::BAD_GATEWAY, String::new());
        assert!(empty.is_retryable());
        assert!(empty.to_string().contains("Bad Gateway"));
    }
}
