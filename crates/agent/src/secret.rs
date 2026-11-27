//! Secret values in stored configuration: `env:NAME` references, masking for API responses,
//! and "keep the existing value" merging for updates.
//!
//! A stored secret is either a literal (`sk-…`) or a reference (`env:OPENAI_API_KEY`) that is
//! resolved from the process environment at use time, so keys never have to touch the database.

use std::collections::BTreeMap;

use serde::Serialize;

/// Prefix of an environment-variable reference.
pub const ENV_PREFIX: &str = "env:";

const MASK_KEEP: usize = 4;
const MASK_MIN_LEN: usize = 12;
const SHORT_MASK: &str = "••••";

/// Where a secret comes from (shown in UIs instead of the value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretSource {
    None,
    Inline,
    Env,
}

pub fn source(value: Option<&str>) -> SecretSource {
    match value {
        None | Some("") => SecretSource::None,
        Some(v) if env_name(v).is_some() => SecretSource::Env,
        Some(_) => SecretSource::Inline,
    }
}

/// `Some("NAME")` for `env:NAME`.
pub fn env_name(value: &str) -> Option<&str> {
    value
        .strip_prefix(ENV_PREFIX)
        .map(str::trim)
        .filter(|n| !n.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("environment variable {0} is not set")]
    MissingEnv(String),
}

/// Resolves `env:NAME` from the environment; literals are returned unchanged.
pub fn resolve(value: &str) -> Result<String, SecretError> {
    resolve_with(value, |n| std::env::var(n).ok())
}

/// [`resolve`] with an explicit lookup (for tests and sandboxes).
pub fn resolve_with(
    value: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, SecretError> {
    match env_name(value) {
        Some(name) => lookup(name)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| SecretError::MissingEnv(name.to_string())),
        None => Ok(value.to_string()),
    }
}

/// Resolves every value of a map (headers, stdio env).
pub fn resolve_map(
    map: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, SecretError> {
    map.iter()
        .map(|(k, v)| Ok((k.clone(), resolve(v)?)))
        .collect()
}

/// Display form of a secret: `AIza…9x2Q` for long literals, `••••` for short ones, references
/// (`env:NAME`) unchanged since they are not secret.
pub fn mask(value: &str) -> String {
    if value.is_empty() || env_name(value).is_some() {
        return value.to_string();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() < MASK_MIN_LEN {
        return SHORT_MASK.to_string();
    }
    let head: String = chars[..MASK_KEEP].iter().collect();
    let tail: String = chars[chars.len() - MASK_KEEP..].iter().collect();
    format!("{head}…{tail}")
}

/// Whether a header / environment variable name likely carries a credential.
pub fn is_sensitive_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "token",
        "key",
        "secret",
        "password",
        "credential",
    ]
    .iter()
    .any(|s| n.contains(s))
}

/// Masks the values of sensitive entries (by name); other values are shown as is.
pub fn mask_map(map: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.iter()
        .map(|(k, v)| {
            let shown = if is_sensitive_name(k) {
                mask(v)
            } else {
                v.clone()
            };
            (k.clone(), shown)
        })
        .collect()
}

/// Update semantics for a single secret: `None`, `""` or the masked form of the current value
/// (a UI echoing back what it was shown) keep `current`; anything else replaces it.
pub fn merge(current: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match incoming {
        None | Some("") => current.map(str::to_string),
        Some(v) if current.is_some_and(|c| mask(c) == v && c != v) => current.map(str::to_string),
        Some(v) => Some(v.to_string()),
    }
}

/// Update semantics for a map of possibly-secret values. `incoming = None` keeps the whole map.
/// Otherwise `incoming` defines the key set; per key, an empty or masked value keeps the
/// current value.
pub fn merge_map(
    current: &BTreeMap<String, String>,
    incoming: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    let Some(incoming) = incoming else {
        return current.clone();
    };
    incoming
        .iter()
        .filter_map(|(k, v)| {
            // A new key with an empty value has nothing to keep and is dropped.
            let merged = merge(current.get(k).map(String::as_str), Some(v))?;
            Some((k.clone(), merged))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_long_short_and_references() {
        assert_eq!(mask("AIzaSyD-0123456789-9x2Q"), "AIza…9x2Q");
        assert_eq!(mask("short"), "••••");
        assert_eq!(mask("env:GEMINI_API_KEY"), "env:GEMINI_API_KEY");
        assert_eq!(mask(""), "");
        // Multi-byte characters are kept whole.
        assert_eq!(mask("ああああいいいいうううう"), "ああああ…うううう");
    }

    #[test]
    fn resolves_env_references() {
        let lookup = |n: &str| (n == "SET").then(|| "v4lue".to_string());
        assert_eq!(resolve_with("env:SET", lookup).unwrap(), "v4lue");
        assert_eq!(resolve_with("env: SET ", lookup).unwrap(), "v4lue");
        assert_eq!(resolve_with("literal", lookup).unwrap(), "literal");
        assert_eq!(
            resolve_with("env:UNSET", lookup),
            Err(SecretError::MissingEnv("UNSET".into()))
        );
        // `env:` alone is not a reference.
        assert_eq!(resolve_with("env:", lookup).unwrap(), "env:");
        assert_eq!(source(Some("env:X")), SecretSource::Env);
        assert_eq!(source(Some("sk")), SecretSource::Inline);
        assert_eq!(source(Some("")), SecretSource::None);
    }

    #[test]
    fn merge_keeps_on_empty_missing_or_masked() {
        let cur = Some("sk-live-abcdefghijkl");
        assert_eq!(merge(cur, None).as_deref(), cur);
        assert_eq!(merge(cur, Some("")).as_deref(), cur);
        assert_eq!(merge(cur, Some("sk-l…ijkl")).as_deref(), cur);
        assert_eq!(merge(cur, Some("new-key")).as_deref(), Some("new-key"));
        assert_eq!(merge(None, Some("")), None);
        assert_eq!(merge(None, Some("k")).as_deref(), Some("k"));
        // A short secret is shown as •••• and that echo keeps it too.
        assert_eq!(merge(Some("abc"), Some("••••")).as_deref(), Some("abc"));
    }

    #[test]
    fn map_masking_and_merge() {
        let cur = BTreeMap::from([
            (
                "Authorization".to_string(),
                "Bearer abcdefghijklmnop".to_string(),
            ),
            ("X-Org".to_string(), "acme".to_string()),
        ]);
        let shown = mask_map(&cur);
        assert_eq!(shown["Authorization"], "Bear…mnop");
        assert_eq!(shown["X-Org"], "acme");
        // Echoing the masked map back changes nothing; dropping a key removes it.
        assert_eq!(merge_map(&cur, Some(&shown)), cur);
        let only_auth = BTreeMap::from([("Authorization".to_string(), String::new())]);
        assert_eq!(
            merge_map(&cur, Some(&only_auth)),
            BTreeMap::from([(
                "Authorization".to_string(),
                "Bearer abcdefghijklmnop".to_string()
            )])
        );
        assert_eq!(merge_map(&cur, None), cur);
    }
}
