//! `scan_security` — agent loop tool that scans a URL+HTML for threats.
//!
//! Delegates to [`crate::security::run_full_scan`] so this tool path and
//! the IPC `scan_page` handler stay in lockstep on heuristics + the
//! Qwen2 model classifier. Both surfaces produce the same trust score
//! and the same threat list for a given input.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    html: String,
    url: String,
}

#[derive(Debug, Serialize)]
struct Result {
    score: u8,
    threats: Vec<Threat>,
}

#[derive(Debug, Serialize)]
struct Threat {
    kind: String,
    description: String,
    severity: String,
}

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: "scan_security",
        description: "Scan HTML + URL for security threats and return a trust score (0-100)",
        parameters: json!({
            "type": "object",
            "properties": {
                "html": { "type": "string", "description": "Raw HTML of the page" },
                "url": { "type": "string", "description": "URL of the page" }
            },
            "required": ["html", "url"]
        }),
    }
}

pub async fn execute(args: serde_json::Value) -> std::result::Result<serde_json::Value, LupusError> {
    let params: Params = serde_json::from_value(args)
        .map_err(|e| LupusError::ToolError { tool: "scan_security".into(), message: e.to_string() })?;

    tracing::debug!("scan_security: url={}", params.url);

    let (score, indicators) = crate::security::run_full_scan(&params.url, &params.html).await;

    let threats: Vec<Threat> = indicators
        .into_iter()
        .map(|t| Threat {
            kind: t.kind,
            description: t.description,
            severity: t.severity,
        })
        .collect();

    let result = Result { score, threats };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}
