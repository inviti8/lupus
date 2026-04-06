//! `extract_content` — extract clean text, title, and summary from HTML.

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
    let content = strip_tags(&params.html);
    let summary = if content.len() > 300 {
        format!("{}...", &content[..297])
    } else {
        content.clone()
    };

    let result = Result {
        title,
        summary,
        content,
        keywords: Vec::new(), // TODO: keyword extraction via model
        content_type: classify_content(&params.html),
    };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}

fn extract_title(html: &str) -> String {
    if let Some(start) = html.find("<title") {
        if let Some(tag_end) = html[start..].find('>') {
            let after = start + tag_end + 1;
            if let Some(close) = html[after..].find("</title>") {
                return html[after..after + close].trim().to_string();
            }
        }
    }
    String::new()
}

fn strip_tags(html: &str) -> String {
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
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn classify_content(html: &str) -> String {
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
