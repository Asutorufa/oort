use bytes::Bytes;
use futures_util::Stream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::pin::Pin;

use crate::Error;

pub(crate) type BytesStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;

pub(crate) fn bytes_stream(response: reqwest::Response) -> BytesStream {
    Box::pin(response.bytes_stream())
}

/// Checks if the HTTP response succeeded; otherwise reads the body and returns `Error::Api`.
pub(crate) async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response, Error> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(Error::from_http_response(status, body))
    }
}

/// Normalizes a base URL by trimming whitespace and trailing slashes, falling back to `default`.
pub(crate) fn normalize_base_url<'a>(base_url: &'a str, default: &'a str) -> &'a str {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        default.trim_end_matches('/')
    } else {
        trimmed
    }
}

/// Inserts a custom HTTP header into a `HeaderMap`, returning `Error::InvalidConfig` on invalid names/values.
pub(crate) fn insert_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
) -> Result<(), Error> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| Error::InvalidConfig(format!("invalid header name '{name}': {e}")))?;
    let header_value = HeaderValue::from_str(value)
        .map_err(|e| Error::InvalidConfig(format!("invalid header value for '{name}': {e}")))?;
    headers.insert(header_name, header_value);
    Ok(())
}

/// Applies extra custom headers to a `reqwest::RequestBuilder`.
pub(crate) fn apply_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
}
