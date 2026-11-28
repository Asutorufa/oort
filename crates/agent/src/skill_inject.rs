//! Explicit skill invocation inside user messages.
//!
//! The application appends `<active_skill name="…">body</active_skill>` blocks to a user
//! message (never to the system prompt or older messages). When that message enters the
//! history, [`dedupe`] keeps the first body of each skill per epoch and turns repeats into
//! `<skill_already_loaded name="…" />`, so a skill invoked twice does not cost its body twice.

use std::collections::BTreeSet;

use oort_llm::{ContentPart, Message};

const OPEN: &str = "<active_skill name=\"";
const CLOSE: &str = "</active_skill>";

/// Wraps a skill body for a user message.
pub fn active_skill_block(name: &str, body: &str) -> String {
    format!("{OPEN}{name}\">\n{body}\n{CLOSE}")
}

fn dedupe_text(text: &str, loaded: &mut BTreeSet<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let (Some(name_end), Some(close)) = (after.find('"'), after.find(CLOSE)) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..name_end];
        let block_end = start + OPEN.len() + close + CLOSE.len();
        if loaded.insert(name.to_string()) {
            out.push_str(&rest[start..block_end]);
        } else {
            out.push_str(&format!("<skill_already_loaded name=\"{name}\" />"));
        }
        rest = &rest[block_end..];
    }
    out.push_str(rest);
    out
}

/// Deduplicates skill bodies in a message about to be appended, recording new ones.
pub fn dedupe(message: &mut Message, loaded: &mut BTreeSet<String>) {
    for part in &mut message.content {
        if let ContentPart::Text { text } = part
            && text.contains(OPEN)
        {
            *text = dedupe_text(text, loaded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_body_per_epoch_wins() {
        let mut loaded = BTreeSet::new();
        let a = format!("{}\n\nreview this", active_skill_block("review", "STEPS"));
        assert_eq!(dedupe_text(&a, &mut loaded), a);
        assert!(loaded.contains("review"));
        let again = dedupe_text(&a, &mut loaded);
        assert_eq!(
            again,
            "<skill_already_loaded name=\"review\" />\n\nreview this"
        );
        // Two different skills in one message; one new, one known.
        let both = format!(
            "{}{}",
            active_skill_block("review", "X"),
            active_skill_block("triage", "Y")
        );
        let out = dedupe_text(&both, &mut loaded);
        assert!(out.starts_with("<skill_already_loaded name=\"review\" />"));
        assert!(out.contains("<active_skill name=\"triage\">\nY\n</active_skill>"));
        // Unterminated blocks are left alone.
        assert_eq!(
            dedupe_text("<active_skill name=\"x\">oops", &mut loaded),
            "<active_skill name=\"x\">oops"
        );
    }
}
