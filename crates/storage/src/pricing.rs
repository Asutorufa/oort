//! Model unit prices (USD per 1M tokens) and the cost / cache-savings calculation.
//!
//! Resolution order: user override → built-in table (exact, then longest prefix, per vendor)
//! → `ollama` is free → unknown (`None`, billed as $0 and reported as unpriced).
//!
//! The built-in table is a list-price snapshot (2026-11) and only an estimate: tiers, batch or
//! off-peak discounts are not modelled. Users correct it with overrides.

use oort_agent::ProviderKind;
use oort_llm::Usage;
use serde::{Deserialize, Serialize};

/// The tokens of one call, split so that no token is counted twice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    /// Input that was neither read from nor written to the cache.
    pub input: u64,
    pub cached_input: u64,
    pub cache_write: u64,
    /// Visible output (without thinking).
    pub output: u64,
    pub thinking: u64,
}

impl TokenCounts {
    /// Normalizes a provider's usage report. Anthropic's `input_tokens` already excludes cache
    /// reads and writes; the other protocols include them. Gemini counts thoughts outside the
    /// candidate tokens; the others include reasoning in the output tokens.
    pub fn from_usage(u: &Usage, kind: ProviderKind) -> Self {
        let cached = u.cache_read_tokens.unwrap_or(0);
        let write = u.cache_creation_tokens.unwrap_or(0);
        let thinking = u.reasoning_tokens.unwrap_or(0);
        let input = match kind {
            ProviderKind::Anthropic => u.input_tokens,
            _ => u.input_tokens.saturating_sub(cached + write),
        };
        let (output, thinking) = match kind {
            ProviderKind::Gemini => (u.output_tokens, thinking),
            _ => {
                let thinking = thinking.min(u.output_tokens);
                (u.output_tokens - thinking, thinking)
            }
        };
        Self {
            input,
            cached_input: cached,
            cache_write: write,
            output,
            thinking,
        }
    }

    pub fn total(&self) -> u64 {
        self.input + self.cached_input + self.cache_write + self.output + self.thinking
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub cached_input: f64,
    pub cache_write: f64,
    pub output: f64,
}

impl Price {
    const fn new(input: f64, cached_input: f64, cache_write: f64, output: f64) -> Self {
        Self {
            input,
            cached_input,
            cache_write,
            output,
        }
    }

    pub const FREE: Price = Price::new(0.0, 0.0, 0.0, 0.0);
}

/// A user-defined price. Missing cache prices fall back to the input price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingOverride {
    /// Exact model id, or a prefix followed by `*`.
    pub model_pattern: String,
    pub input_per_m: Option<f64>,
    pub cached_input_per_m: Option<f64>,
    pub cache_write_per_m: Option<f64>,
    pub output_per_m: Option<f64>,
    #[serde(default, skip_deserializing)]
    pub updated_at: Option<String>,
}

impl PricingOverride {
    pub fn price(&self) -> Price {
        let input = self.input_per_m.unwrap_or(0.0);
        Price {
            input,
            cached_input: self.cached_input_per_m.unwrap_or(input),
            cache_write: self.cache_write_per_m.unwrap_or(input),
            output: self.output_per_m.unwrap_or(0.0),
        }
    }

    pub fn matches(&self, model: &str) -> bool {
        match self.model_pattern.strip_suffix('*') {
            Some(prefix) => model.starts_with(prefix),
            None => self.model_pattern == model,
        }
    }
}

/// Anthropic: cache reads 0.1× input unless listed, 5-minute cache writes 1.25× input.
const ANTHROPIC: &[(&str, Price)] = &[
    ("claude-fable-5-2", Price::new(8.0, 0.20, 10.0, 40.0)),
    ("claude-fable-5-1", Price::new(10.0, 1.0, 12.5, 50.0)),
    ("claude-fable-5", Price::new(10.0, 1.0, 12.5, 50.0)),
    ("claude-opus-5-5", Price::new(4.0, 0.20, 5.0, 20.0)),
    ("claude-opus-5", Price::new(5.0, 0.50, 6.25, 25.0)),
    ("claude-opus-4-8", Price::new(5.0, 0.50, 6.25, 25.0)),
    ("claude-opus-4-7", Price::new(5.0, 0.50, 6.25, 25.0)),
    ("claude-opus-4-6", Price::new(5.0, 0.50, 6.25, 25.0)),
    ("claude-opus-4-5", Price::new(5.0, 0.50, 6.25, 25.0)),
    ("claude-opus-4-1", Price::new(15.0, 1.50, 18.75, 75.0)),
    ("claude-opus-4", Price::new(15.0, 1.50, 18.75, 75.0)),
    ("claude-sonnet-5-5", Price::new(2.0, 0.10, 2.5, 10.0)),
    ("claude-sonnet-5", Price::new(2.0, 0.20, 2.5, 10.0)),
    ("claude-sonnet-4-6", Price::new(3.0, 0.30, 3.75, 15.0)),
    ("claude-sonnet-4-5", Price::new(3.0, 0.30, 3.75, 15.0)),
    ("claude-sonnet-4", Price::new(3.0, 0.30, 3.75, 15.0)),
    ("claude-haiku-5-5", Price::new(0.10, 0.01, 0.125, 0.50)),
    ("claude-haiku-4-5", Price::new(1.0, 0.10, 1.25, 5.0)),
];

const OPENAI: &[(&str, Price)] = &[
    ("gpt-5.5", Price::new(5.0, 0.50, 5.0, 30.0)),
    ("gpt-5.2", Price::new(1.75, 0.175, 1.75, 14.0)),
    ("gpt-5.1", Price::new(1.25, 0.125, 1.25, 10.0)),
    ("gpt-5", Price::new(1.25, 0.125, 1.25, 10.0)),
    ("gpt-5-mini", Price::new(0.25, 0.025, 0.25, 2.0)),
    ("gpt-5-nano", Price::new(0.05, 0.005, 0.05, 0.40)),
    ("o3", Price::new(2.0, 0.50, 2.0, 8.0)),
    ("o4-mini", Price::new(1.10, 0.275, 1.10, 4.40)),
];

const GEMINI: &[(&str, Price)] = &[
    ("gemini-3.1-pro", Price::new(2.0, 0.20, 2.0, 12.0)),
    ("gemini-3-flash", Price::new(0.50, 0.05, 0.50, 3.0)),
    ("gemini-2.5-pro", Price::new(1.25, 0.125, 1.25, 10.0)),
    ("gemini-2.5-flash", Price::new(0.30, 0.03, 0.30, 2.50)),
];

/// Off-peak rates (weekday peak hours are billed at twice these). The older names are served,
/// and billed, as the current Flash model.
const DEEPSEEK_FLASH: Price = Price::new(0.15, 0.003, 0.15, 0.60);
const DEEPSEEK: &[(&str, Price)] = &[
    ("deepseek-v4-pro", Price::new(0.66, 0.022, 0.66, 1.98)),
    ("deepseek-v4-flash", DEEPSEEK_FLASH),
    ("deepseek-flash", DEEPSEEK_FLASH),
    ("deepseek-chat", DEEPSEEK_FLASH),
    ("deepseek-reasoner", DEEPSEEK_FLASH),
];

const QWEN: &[(&str, Price)] = &[("qwen3-max", Price::new(0.78, 0.156, 0.78, 3.90))];

fn table(vendor: &str) -> &'static [(&'static str, Price)] {
    match vendor {
        "anthropic" => ANTHROPIC,
        "openai" => OPENAI,
        "gemini" => GEMINI,
        "deepseek" => DEEPSEEK,
        "qwen" => QWEN,
        _ => &[],
    }
}

/// Whether `model` is a variant (snapshot date, preview, …) of the table entry `base`. A short
/// number after the base is a different model version (`claude-opus-4` vs `claude-opus-4-9`),
/// and so is a dotted one (`gpt-5` vs `gpt-5.9`).
fn is_variant_of(model: &str, base: &str) -> bool {
    let Some(rest) = model.strip_prefix(base) else {
        return false;
    };
    let Some(tail) = rest.strip_prefix(['-', '@', ':']) else {
        return false;
    };
    let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
    let is_minor_version =
        (1..=2).contains(&digits) && matches!(tail.as_bytes().get(digits), None | Some(b'-'));
    // `-09-2025`-style preview dates only follow a word (`-preview-09-2025`), so a leading
    // short number is always a version.
    !is_minor_version
}

fn builtin(vendor: &str, model: &str) -> Option<Price> {
    let table = table(vendor);
    table
        .iter()
        .find(|(name, _)| *name == model)
        .or_else(|| {
            table
                .iter()
                .filter(|(name, _)| is_variant_of(model, name))
                .max_by_key(|(name, _)| name.len())
        })
        .map(|(_, price)| *price)
}

/// `None`: no price is known (the UI asks the user to set one).
pub fn resolve(overrides: &[PricingOverride], vendor: &str, model: &str) -> Option<Price> {
    // An exact override beats a prefix one; among prefixes the longest wins.
    overrides
        .iter()
        .filter(|o| o.matches(model))
        .max_by_key(|o| (!o.model_pattern.ends_with('*'), o.model_pattern.len()))
        .map(PricingOverride::price)
        .or_else(|| builtin(vendor, model))
        .or_else(|| (vendor == "ollama").then_some(Price::FREE))
}

/// `(actual cost, what caching saved)` in USD. Thinking is billed as output.
pub fn cost(price: Price, t: TokenCounts) -> (f64, f64) {
    let out = (t.output + t.thinking) as f64 * price.output;
    let actual = (t.input as f64 * price.input
        + t.cached_input as f64 * price.cached_input
        + t.cache_write as f64 * price.cache_write
        + out)
        / 1_000_000.0;
    let without_cache =
        ((t.input + t.cached_input + t.cache_write) as f64 * price.input + out) / 1_000_000.0;
    (actual, (without_cache - actual).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn over(pattern: &str, input: f64, output: f64) -> PricingOverride {
        PricingOverride {
            model_pattern: pattern.into(),
            input_per_m: Some(input),
            cached_input_per_m: None,
            cache_write_per_m: None,
            output_per_m: Some(output),
            updated_at: None,
        }
    }

    #[test]
    fn override_beats_builtin_beats_ollama_free() {
        let sonnet = resolve(&[], "anthropic", "claude-sonnet-4-5").unwrap();
        assert_eq!((sonnet.input, sonnet.output), (3.0, 15.0));

        let custom = [over("claude-sonnet-4-5", 1.0, 2.0)];
        let p = resolve(&custom, "anthropic", "claude-sonnet-4-5").unwrap();
        assert_eq!((p.input, p.cached_input, p.output), (1.0, 1.0, 2.0));

        assert_eq!(resolve(&[], "ollama", "llama4"), Some(Price::FREE));
        assert_eq!(
            resolve(&[over("llama4", 5.0, 6.0)], "ollama", "llama4")
                .unwrap()
                .input,
            5.0
        );
        assert_eq!(resolve(&[], "custom", "claude-sonnet-4-5"), None);
        assert_eq!(resolve(&[], "openai", "my-finetune"), None);
    }

    #[test]
    fn exact_override_beats_prefix_and_longest_prefix_wins() {
        let all = [
            over("gpt-*", 1.0, 1.0),
            over("gpt-5*", 2.0, 2.0),
            over("gpt-5-mini", 3.0, 3.0),
        ];
        assert_eq!(resolve(&all, "openai", "gpt-5-mini").unwrap().input, 3.0);
        assert_eq!(resolve(&all, "openai", "gpt-5.2").unwrap().input, 2.0);
        assert_eq!(resolve(&all, "openai", "gpt-4o").unwrap().input, 1.0);
    }

    #[test]
    fn builtin_prefix_matching_respects_versions() {
        let input = |vendor, model| resolve(&[], vendor, model).map(|p| p.input);
        assert_eq!(input("anthropic", "claude-opus-4-20250514"), Some(15.0));
        assert_eq!(input("anthropic", "claude-opus-4-5-20251101"), Some(5.0));
        assert_eq!(input("anthropic", "claude-opus-4-9"), None);
        assert_eq!(input("openai", "gpt-5-mini-2025-08-07"), Some(0.25));
        assert_eq!(input("openai", "gpt-5-2025-08-07"), Some(1.25));
        assert_eq!(input("openai", "gpt-5.9"), None);
        assert_eq!(
            input("gemini", "gemini-2.5-flash-preview-09-2025"),
            Some(0.30)
        );
    }

    #[test]
    fn cost_and_cache_savings() {
        let price = Price::new(3.0, 0.3, 3.75, 15.0);
        let tokens = TokenCounts {
            input: 1_000_000,
            cached_input: 2_000_000,
            cache_write: 400_000,
            output: 100_000,
            thinking: 100_000,
        };
        let (actual, saved) = cost(price, tokens);
        assert!((actual - (3.0 + 0.6 + 1.5 + 3.0)).abs() < 1e-9);
        // Without a cache every input token costs the input price.
        assert!((saved - ((3.4 * 3.0 + 3.0) - actual)).abs() < 1e-9);
        // A cache write that was never read costs more than no cache: savings never go negative.
        let write_only = TokenCounts {
            cache_write: 1_000_000,
            ..Default::default()
        };
        assert_eq!(cost(price, write_only), (3.75, 0.0));
    }

    #[test]
    fn usage_is_normalized_per_protocol() {
        let u = Usage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: None,
            reasoning_tokens: Some(20),
            cache_read_tokens: Some(60),
            cache_creation_tokens: None,
        };
        let openai = TokenCounts::from_usage(&u, ProviderKind::Openai);
        assert_eq!((openai.input, openai.output, openai.thinking), (40, 30, 20));
        let anthropic = TokenCounts::from_usage(&u, ProviderKind::Anthropic);
        assert_eq!((anthropic.input, anthropic.cached_input), (100, 60));
        let gemini = TokenCounts::from_usage(&u, ProviderKind::Gemini);
        assert_eq!((gemini.input, gemini.output, gemini.thinking), (40, 50, 20));
    }
}
