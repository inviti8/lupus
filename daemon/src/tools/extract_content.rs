//! `extract_content` — extract clean text, title, and summary from HTML.
//!
//! ## v0.2 extraction pipeline (quick-and-dirty)
//!
//! The v0.1 implementation did naive character-level `<` / `>` tag
//! stripping which preserved inline `<script>` / `<style>` content as
//! visible "text" — on a real Wikipedia page the "summary" field was
//! 300 chars of CSS classnames and JavaScript code, not the article.
//! The joinner would then correctly conclude "this tool didn't work"
//! and abstain.
//!
//! This version:
//!
//! 1. **Pre-strips noise tags** — `<script>`, `<style>`, `<noscript>`
//!    are removed (with their contents) before any other processing.
//!    `<nav>`, `<header>`, `<footer>` are also dropped because they
//!    dominate the top-of-DOM and bury article text.
//!
//! 2. **Summary prefers structured signals**:
//!    - First choice: `<meta name="description">` or
//!      `<meta property="og:description">` content.
//!    - Second choice: the first `<p>` element with > 80 chars of
//!      actual text (real sentences, not nav links).
//!    - Fallback: the first 400 chars of the stripped body.
//!
//! 3. **Content is the full stripped text** — unchanged from v0.1.
//!    Intended for downstream tools that want the full article text.
//!
//! The whole thing is still regex-free byte scanning — no new deps.
//! A proper Readability port is a separate future workstream; this
//! gets observation quality from "useless" to "joinner can work with
//! it" in <150 lines.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    html: String,
    #[serde(default = "default_format")]
    format: String,
}

fn default_format() -> String { "full".into() }

#[derive(Debug, Serialize)]
struct Result {
    title: String,
    summary: String,
    content: String,
    keywords: Vec<String>,
    content_type: String,
}

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: "extract_content",
        description: "Extract clean text, title, summary, and keywords from raw HTML",
        parameters: json!({
            "type": "object",
            "properties": {
                "html": { "type": "string", "description": "Raw HTML to extract from" },
                "format": { "type": "string", "description": "'full' or 'summary' (default: full)" }
            },
            "required": ["html"]
        }),
    }
}

pub async fn execute(args: serde_json::Value) -> std::result::Result<serde_json::Value, LupusError> {
    let params: Params = serde_json::from_value(args)
        .map_err(|e| LupusError::ToolError { tool: "extract_content".into(), message: e.to_string() })?;

    tracing::debug!("extract_content: format={}, html_len={}", params.format, params.html.len());

    let title = extract_title(&params.html);

    // Drop noise tags (with content) before any text extraction.
    let cleaned = strip_noise_tags(&params.html);

    // Summary priority: meta description → first meaningful <p> → fallback.
    let summary = extract_meta_description(&params.html)
        .or_else(|| extract_first_paragraph(&cleaned))
        .unwrap_or_else(|| fallback_summary(&cleaned));

    let content = strip_tags(&cleaned);

    let result = Result {
        title,
        summary,
        content,
        keywords: extract_meta_keywords(&params.html),
        content_type: classify_content(&params.html),
    };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}

// ─── Title ─────────────────────────────────────────────────────────────────

pub(crate) fn extract_title(html: &str) -> String {
    if let Some(start) = html.find("<title") {
        if let Some(tag_end) = html[start..].find('>') {
            let after = start + tag_end + 1;
            if let Some(close) = html[after..].find("</title>") {
                return decode_entities(html[after..after + close].trim());
            }
        }
    }
    String::new()
}

// ─── Noise stripping ───────────────────────────────────────────────────────

/// Remove `<script>`, `<style>`, `<noscript>`, `<nav>`, `<header>`,
/// `<footer>` tags INCLUDING their content. Runs before any other
/// processing so the rest of the pipeline only sees "content" DOM.
pub(crate) fn strip_noise_tags(html: &str) -> String {
    const NOISE: &[&str] = &["script", "style", "noscript", "nav", "header", "footer"];
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let lower: String = html.to_lowercase();
    let lower_bytes = lower.as_bytes();
    let mut i = 0;
    'outer: while i < bytes.len() {
        if bytes[i] == b'<' {
            for tag in NOISE {
                let open = format!("<{tag}");
                let close = format!("</{tag}>");
                if lower_bytes[i..].starts_with(open.as_bytes()) {
                    // Found an opening noise tag; skip until the matching close.
                    if let Some(end_rel) = lower[i..].find(&close) {
                        i += end_rel + close.len();
                        continue 'outer;
                    } else {
                        // Unclosed — drop the rest.
                        return out;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ─── Summary candidates ────────────────────────────────────────────────────

/// `<meta name="description" content="...">` or og:description.
pub(crate) fn extract_meta_description(html: &str) -> Option<String> {
    let patterns = [
        "name=\"description\"",
        "name='description'",
        "property=\"og:description\"",
        "property='og:description'",
    ];
    let lower = html.to_lowercase();
    for pat in &patterns {
        if let Some(pos) = lower.find(pat) {
            // Search within a 500-char window after the match for content="...".
            let window_end = (pos + 500).min(html.len());
            let window = &html[pos..window_end];
            if let Some(c_start) = window.find("content=") {
                let after = c_start + "content=".len();
                let rest = &window[after..];
                let (quote, rest) = match rest.chars().next() {
                    Some('"') => ('"', &rest[1..]),
                    Some('\'') => ('\'', &rest[1..]),
                    _ => continue,
                };
                if let Some(end) = rest.find(quote) {
                    let content = decode_entities(rest[..end].trim());
                    if content.len() > 10 {
                        return Some(content);
                    }
                }
            }
        }
    }
    None
}

/// First `<p>` element whose inner text has at least 80 chars of real content.
/// Skips short navigation-style paragraphs and takes the first substantive one.
pub(crate) fn extract_first_paragraph(html: &str) -> Option<String> {
    let mut cursor = 0usize;
    while let Some(rel) = html[cursor..].find("<p") {
        let p_start = cursor + rel;
        let after_open = match html[p_start..].find('>') {
            Some(g) => p_start + g + 1,
            None => break,
        };
        let after_close = match html[after_open..].find("</p>") {
            Some(c) => after_open + c,
            None => break,
        };
        let inner = &html[after_open..after_close];
        let text = strip_tags(inner);
        if text.len() >= 80 {
            return Some(truncate_chars(&text, 400));
        }
        cursor = after_close + 4;
    }
    None
}

fn fallback_summary(cleaned: &str) -> String {
    let text = strip_tags(cleaned);
    truncate_chars(&text, 400)
}

// ─── Raw body + keyword/classifier helpers ─────────────────────────────────

pub(crate) fn strip_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    let collapsed: String = result.split_whitespace().collect::<Vec<_>>().join(" ");
    decode_entities(&collapsed)
}

fn extract_meta_keywords(html: &str) -> Vec<String> {
    let lower = html.to_lowercase();
    if let Some(pos) = lower.find("name=\"keywords\"") {
        let region = &html[pos..std::cmp::min(pos + 500, html.len())];
        if let Some(cstart) = region.find("content=\"") {
            let after = cstart + 9;
            if let Some(cend) = region[after..].find('"') {
                return region[after..after + cend]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

pub(crate) fn classify_content(html: &str) -> String {
    let lower = html.to_lowercase();
    if lower.contains("<article") {
        "article".into()
    } else if lower.contains("class=\"gallery\"") || lower.contains("class=\"grid\"") {
        "gallery".into()
    } else if lower.contains("class=\"product\"") || lower.contains("add-to-cart") {
        "shop".into()
    } else {
        "page".into()
    }
}

// ─── Small utilities ───────────────────────────────────────────────────────

/// Decode a small set of common HTML entities. Not comprehensive but
/// covers what typically shows up in article text.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_script_and_style_content() {
        let html = r#"<html><head><style>body { color: red; }</style></head>
<body><script>alert('x')</script><p>Real content here that is long enough to be meaningful to the reader.</p></body></html>"#;
        let cleaned = strip_noise_tags(html);
        assert!(!cleaned.contains("alert"));
        assert!(!cleaned.contains("color: red"));
        assert!(cleaned.contains("Real content"));
    }

    #[test]
    fn prefers_meta_description_for_summary() {
        let html = r#"<html><head>
<meta name="description" content="The wolf (Canis lupus) is a large canine native to Eurasia and North America.">
</head><body><p>Short para.</p></body></html>"#;
        let summary = extract_meta_description(html).expect("should find meta desc");
        assert!(summary.contains("Canis lupus"));
    }

    #[test]
    fn og_description_also_works() {
        let html = r#"<html><head>
<meta property="og:description" content="A wolf article with a long enough description to matter.">
</head></html>"#;
        let summary = extract_meta_description(html).expect("should find og:description");
        assert!(summary.contains("wolf article"));
    }

    #[test]
    fn first_paragraph_skips_short_ones() {
        let html = r#"<p>Nav.</p><p>Still short</p>
<p>This is the first paragraph with real substantive content that exceeds the eighty character minimum threshold for selection.</p>"#;
        let summary = extract_first_paragraph(html).expect("should find long p");
        assert!(summary.contains("substantive content"));
        assert!(!summary.contains("Nav."));
    }

    #[test]
    fn end_to_end_wikipedia_shape() {
        // Miniature mock of the kind of garbage real Wikipedia serves.
        let html = r#"<html><head>
<title>Wolf - Wikipedia</title>
<meta name="description" content="The wolf (Canis lupus) is a large canine native to Eurasia and North America.">
<script>var wgTitle='Wolf';</script>
<style>.mw-body{margin:0}</style>
</head>
<body>
<nav>Main page | Contents | Current events</nav>
<article>
<p>The wolf (Canis lupus), also known as the grey wolf or gray wolf, is a large canine native to Eurasia and North America.</p>
</article>
</body></html>"#;
        let cleaned = strip_noise_tags(html);
        assert!(!cleaned.contains("var wgTitle"));
        assert!(!cleaned.contains(".mw-body"));
        assert!(!cleaned.contains("Main page |"));
        let summary = extract_meta_description(html).expect("meta description");
        assert!(summary.contains("Canis lupus"));
        let title = extract_title(html);
        assert_eq!(title, "Wolf - Wikipedia");
    }

    #[test]
    fn decodes_common_entities() {
        assert_eq!(decode_entities("Rock &amp; roll"), "Rock & roll");
        assert_eq!(decode_entities("&quot;hello&quot;"), "\"hello\"");
        assert_eq!(decode_entities("it&#39;s"), "it's");
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        let s = "café".repeat(20); // multi-byte chars
        let t = truncate_chars(&s, 15);
        assert!(t.is_char_boundary(t.len()));
        assert!(t.ends_with('…') || t.len() <= 15);
    }
}
