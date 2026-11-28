//! `SKILL.md` parsing: a small YAML-frontmatter subset plus a Markdown body.
//!
//! Supported frontmatter (enough for skills, no YAML dependency):
//! `key: value`, quoted values, inline lists `[a, b]`, block lists (`- item` lines),
//! block scalars `|` / `>` and `#` comments.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("missing frontmatter (the file must start with ---)")]
    NoFrontmatter,
    #[error("unterminated frontmatter (no closing ---)")]
    Unterminated,
    #[error("line {0}: expected `key: value`")]
    BadLine(usize),
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let quoted = v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
    if quoted {
        v[1..v.len() - 1].replace("\\\"", "\"").replace("''", "'")
    } else {
        v.to_string()
    }
}

fn strip_comment(line: &str) -> &str {
    // A `#` starts a comment only at the start or after whitespace, outside quotes.
    let mut in_quote: Option<char> = None;
    let mut prev_ws = true;
    for (i, c) in line.char_indices() {
        match (in_quote, c) {
            (None, '"' | '\'') => in_quote = Some(c),
            (Some(q), c) if c == q => in_quote = None,
            (None, '#') if prev_ws => return &line[..i],
            _ => {}
        }
        prev_ws = c.is_whitespace();
    }
    line
}

fn inline_list(v: &str) -> Vec<String> {
    v.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Splits `text` into (frontmatter map, body).
pub fn parse(text: &str) -> Result<(BTreeMap<String, Value>, String), ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err(ParseError::NoFrontmatter);
    }
    let rest: Vec<&str> = lines.collect();
    let end = rest
        .iter()
        .position(|l| l.trim_end() == "---")
        .ok_or(ParseError::Unterminated)?;
    let (front, body) = (&rest[..end], &rest[end + 1..]);

    let mut map = BTreeMap::new();
    let mut i = 0;
    while i < front.len() {
        let line = strip_comment(front[i]);
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let (key, value) = line.split_once(':').ok_or(ParseError::BadLine(i + 2))?;
        let key = key.trim().to_string();
        let value = value.trim();
        i += 1;
        let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty();
        if value == "|" || value == ">" || value == "|-" || value == ">-" {
            let mut block = Vec::new();
            while i < front.len() && indented(front[i]) {
                block.push(front[i].trim());
                i += 1;
            }
            while block.last() == Some(&"") {
                block.pop();
            }
            let joined = if value.starts_with('|') {
                block.join("\n")
            } else {
                block
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            map.insert(key, Value::Str(joined));
        } else if value.is_empty() {
            let mut items = Vec::new();
            while i < front.len() && indented(front[i]) {
                let l = strip_comment(front[i]).trim();
                if let Some(item) = l.strip_prefix('-') {
                    items.push(unquote(item));
                }
                i += 1;
            }
            map.insert(key, Value::List(items));
        } else if value.starts_with('[') {
            map.insert(key, Value::List(inline_list(value)));
        } else {
            map.insert(key, Value::Str(unquote(value)));
        }
    }
    let body = body.join("\n").trim().to_string();
    Ok((map, body))
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::List(_) => None,
        }
    }

    /// Lists as is; a scalar is split on commas (`tags: a, b`).
    pub fn to_list(&self) -> Vec<String> {
        match self {
            Value::List(l) => l.clone(),
            Value::Str(s) => inline_list(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_lists_blocks_and_comments() {
        let text = "---\nname: code-review   # the slash command\ndescription: >\n  Review a diff\n  carefully.\nallowed_tools: [read_file, \"list_dir\"]\ntags:\n  - dev\n  - 'review'\nnote: \"has # hash\"\n---\n\n# Steps\n\n1. Read.\n";
        let (m, body) = parse(text).unwrap();
        assert_eq!(m["name"], Value::Str("code-review".into()));
        assert_eq!(
            m["description"],
            Value::Str("Review a diff carefully.".into())
        );
        assert_eq!(m["allowed_tools"].to_list(), ["read_file", "list_dir"]);
        assert_eq!(m["tags"].to_list(), ["dev", "review"]);
        assert_eq!(m["note"].as_str(), Some("has # hash"));
        assert_eq!(body, "# Steps\n\n1. Read.");
    }

    #[test]
    fn literal_block_keeps_lines_and_errors_are_reported() {
        let (m, _) = parse("---\ndescription: |\n  line one\n  line two\n---\nbody").unwrap();
        assert_eq!(m["description"].as_str(), Some("line one\nline two"));
        assert_eq!(parse("no frontmatter"), Err(ParseError::NoFrontmatter));
        assert_eq!(parse("---\nname: x\n"), Err(ParseError::Unterminated));
        assert_eq!(parse("---\njust text\n---\n"), Err(ParseError::BadLine(2)));
    }
}
