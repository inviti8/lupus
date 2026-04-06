//! `scan_security` — wrapper calling the security scanner model.

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

    // Run heuristic pre-check (available without model)
    let heuristic_threats = crate::security::SecurityScanner::heuristic_scan(&params.url, &params.html);

    let threats: Vec<Threat> = heuristic_threats
        .into_iter()
        .map(|t| Threat {
            kind: t.kind,
            description: t.description,
            severity: t.severity,
        })
        .collect();

    // Calculate score: start at 100, deduct per threat
    let deductions: u8 = threats.iter().map(|t| match t.severity.as_str() {
        "critical" => 40,
        "high" => 25,
        "medium" => 10,
        "low" => 5,
        _ => 5,
    }).sum();
    let score = 100u8.saturating_sub(deductions);

    // TODO: Also run the security model for deeper analysis
    //   let model_result = security_scanner.scan(params).await?;
    //   merge heuristic + model results

    let result = Result { score, threats };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}
