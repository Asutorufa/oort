//! Per-LLM-call measurements: latency, time to first token and a compact request summary.

use oort_llm::{CacheBreakpoint, ChatRequest, Usage};
use serde::{Deserialize, Serialize};

/// Stable 64-bit FNV-1a hashes (hex) of the parts of a request that make up the cache prefix.
/// Equal hashes across calls mean those parts were byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestHashes {
    pub system: String,
    pub tools: String,
    pub messages: String,
    pub full: String,
}

/// What was sent, without the payload (full requests would grow quadratically with history).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestSummary {
    pub model: Option<String>,
    pub messages: usize,
    pub tools: usize,
    /// Size of the canonical JSON request.
    pub bytes: usize,
    pub max_tokens: Option<u32>,
    pub breakpoints: Vec<CacheBreakpoint>,
    pub hash: RequestHashes,
}

impl RequestSummary {
    pub fn of(req: &ChatRequest) -> Self {
        let full = json(req);
        Self {
            model: req.model.clone(),
            messages: req.messages.len(),
            tools: req.tools.len(),
            bytes: full.len(),
            max_tokens: req.max_tokens,
            breakpoints: req.cache_breakpoints.clone(),
            hash: RequestHashes {
                system: fnv1a(json(&req.system).as_bytes()),
                tools: fnv1a(json(&req.tools).as_bytes()),
                messages: fnv1a(json(&req.messages).as_bytes()),
                full: fnv1a(full.as_bytes()),
            },
        }
    }
}

/// One completed LLM call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmCallStats {
    pub turn: u32,
    /// Seq of the assistant message the call produced.
    pub seq: usize,
    /// Time until the first streamed content (text, thinking or tool call).
    pub ttft_ms: Option<u64>,
    /// Request start to end of stream.
    pub latency_ms: u64,
    /// How long the model reasoned before moving on (`None` without reasoning).
    #[serde(default)]
    pub thinking_ms: Option<u64>,
    pub usage: Option<Usage>,
    pub request: RequestSummary,
}

fn fnv1a(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn json<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oort_llm::Message;

    #[test]
    fn hashes_are_stable_and_part_specific() {
        let a = ChatRequest::new(vec![Message::user("hi")]).with_system("sys");
        let mut b = a.clone();
        b.messages.push(Message::assistant("yo"));
        let (sa, sb) = (RequestSummary::of(&a), RequestSummary::of(&b));
        assert_eq!(sa, RequestSummary::of(&a.clone()));
        assert_eq!(sa.hash.system, sb.hash.system);
        assert_eq!(sa.hash.tools, sb.hash.tools);
        assert_ne!(sa.hash.messages, sb.hash.messages);
        assert_eq!(sb.messages, 2);
        assert_eq!(fnv1a(b""), "cbf29ce484222325");
    }
}
