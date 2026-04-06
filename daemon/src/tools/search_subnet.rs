//! `search_subnet` — query cooperative datapod metadata.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    query: String,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Serialize)]
struct Result {
    matches: Vec<DatapodMatch>,
}

#[derive(Debug, Serialize)]
struct DatapodMatch {
    title: String,
    url: String,
    description: String,
    commitment: f64,
}

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: "search_subnet",
        description: "Search cooperative subnet for matching datapod metadata",
        parameters: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "scope": { "type": "string", "description": "Optional subnet scope (e.g. 'hvym')" }
            },
            "required": ["query"]
        }),
    }
}

pub async fn execute(args: serde_json::Value) -> std::result::Result<serde_json::Value, LupusError> {
    let params: Params = serde_json::from_value(args)
        .map_err(|e| LupusError::ToolError { tool: "search_subnet".into(), message: e.to_string() })?;

    tracing::debug!("search_subnet: query={:?} scope={:?}", params.query, params.scope);

    // TODO: Query the cooperative registry for matching datapods
    //   let url = format!("{}/search?q={}", registry_url, params.query);
    //   let response = http_client.get(&url).await?;
    //   parse datapod metadata from response

    let result = Result { matches: vec![] };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}
