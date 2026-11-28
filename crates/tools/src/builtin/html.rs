//! Minimal HTML → Markdown-ish text: enough for reading pages, no dependency.
//! Drops scripts, styles and markup; keeps headings, paragraphs, lists, links and code.

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let Some(end) = end else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[i..];
    let (q, body) = match rest.chars().next()? {
        c @ ('"' | '\'') => (c, &rest[1..]),
        _ => (' ', rest),
    };
    let end = body
        .find(|c: char| c == q || (q == ' ' && c == '>'))
        .unwrap_or(body.len());
    Some(decode_entities(&body[..end]))
}

/// Converts HTML to readable text with light Markdown. `base` resolves relative links.
pub fn to_markdown(html: &str, base: Option<&url::Url>) -> String {
    let mut out = String::new();
    let mut i = 0;
    let lower = html.to_ascii_lowercase();
    let mut link: Option<String> = None;
    let mut in_pre = false;
    let mut title = String::new();
    while i < html.len() {
        let Some(lt) = html[i..].find('<') else {
            push_text(&mut out, &html[i..], in_pre);
            break;
        };
        push_text(&mut out, &html[i..i + lt], in_pre);
        i += lt;
        if lower[i..].starts_with("<!--") {
            i = lower[i..].find("-->").map_or(html.len(), |e| i + e + 3);
            continue;
        }
        let Some(gt) = html[i..].find('>') else { break };
        let tag = &html[i + 1..i + gt];
        i += gt + 1;
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("script" | "style" | "noscript" | "svg" | "template" | "head", false) => {
                if name == "head" {
                    if let Some(t0) = lower[i..].find("<title>") {
                        let start = i + t0 + 7;
                        if let Some(t1) = lower[start..].find("</title>") {
                            title = decode_entities(html[start..start + t1].trim());
                        }
                    }
                }
                let close = format!("</{name}");
                i = lower[i..].find(&close).map_or(html.len(), |e| i + e);
                i = lower[i..].find('>').map_or(html.len(), |e| i + e + 1);
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                let level = name[1..].parse::<usize>().unwrap_or(1);
                out.push_str(&format!("\n\n{} ", "#".repeat(level)));
            }
            (
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "section" | "article"
                | "table" | "ul" | "ol" | "blockquote",
                true,
            ) => out.push_str("\n\n"),
            ("p" | "div" | "section" | "article" | "table" | "blockquote", false) => {
                out.push_str("\n\n")
            }
            ("br" | "tr", _) => out.push('\n'),
            ("li", false) => out.push_str("\n- "),
            ("td" | "th", false) => out.push_str(" | "),
            ("pre", false) => {
                in_pre = true;
                out.push_str("\n\n```\n");
            }
            ("pre", true) => {
                in_pre = false;
                out.push_str("\n```\n\n");
            }
            ("code", _) if !in_pre => out.push('`'),
            ("strong" | "b", _) => out.push_str("**"),
            ("em" | "i", _) => out.push('*'),
            ("a", false) => {
                link = attr(tag, "href").map(|h| match base.and_then(|b| b.join(&h).ok()) {
                    Some(u) => u.to_string(),
                    None => h,
                });
                if link.is_some() {
                    out.push('[');
                }
            }
            ("a", true) => {
                if let Some(h) = link.take() {
                    out.push_str(&format!("]({h})"));
                }
            }
            ("img", _) => {
                if let Some(alt) = attr(tag, "alt").filter(|a| !a.is_empty()) {
                    out.push_str(&format!("[image: {alt}]"));
                }
            }
            _ => {}
        }
    }
    // Collapse runs of blank lines and trailing spaces.
    let mut cleaned = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        cleaned.push_str(l);
        cleaned.push('\n');
    }
    let body = cleaned.trim().to_string();
    if title.is_empty() {
        body
    } else {
        format!("# {title}\n\n{body}")
    }
}

fn push_text(out: &mut String, text: &str, pre: bool) {
    let t = decode_entities(text);
    if pre {
        out.push_str(&t);
        return;
    }
    let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        if !t.is_empty() && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
        return;
    }
    if t.starts_with(char::is_whitespace) && !out.ends_with([' ', '\n', '[']) {
        out.push(' ');
    }
    out.push_str(&collapsed);
    if t.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_markup() {
        let html = r#"<html><head><title>T &amp; U</title><style>x{}</style></head>
            <body><h1>Hello</h1><p>Some <b>bold</b> and <a href="/docs">docs</a>.</p>
            <script>alert(1)</script><ul><li>one</li><li>two</li></ul>
            <pre>let x = 1;
let y = 2;</pre><p>caf&eacute; &#x263A; &lt;tag&gt;</p></body></html>"#;
        let base = url::Url::parse("https://example.com/a/").unwrap();
        let md = to_markdown(html, Some(&base));
        assert!(md.starts_with("# T & U"), "{md}");
        assert!(md.contains("# Hello"));
        assert!(
            md.contains("Some **bold** and [docs](https://example.com/docs)."),
            "{md}"
        );
        assert!(md.contains("- one\n- two"));
        assert!(md.contains("```\nlet x = 1;\nlet y = 2;\n```"));
        assert!(!md.contains("alert") && !md.contains("x{}"));
        assert!(md.contains("☺ <tag>"));
    }
}
