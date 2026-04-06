//! `fetch_page` — fetch page content by URL (HVYM datapod or HTTPS).

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    url: String,
}

#[derive(Debug, Serialize)]
struct Result {
    url: String,
    content_type: String,
    body: String,
    status: String,
}

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: "fetch_page",
        description: "Fetch page content by URL (supports hvym:// datapod URLs and https://)",
        parameters: json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "URL to fetch (hvym or https)" }
            },
            "required": ["url"]
        }),
    }
}

pub async fn execute(args: serde_json::Value) -> std::result::Result<serde_json::Value, LupusError> {
    let params: Params = serde_json::from_value(args)
        .map_err(|e| LupusError::ToolError { tool: "fetch_page".into(), message: e.to_string() })?;

    tracing::debug!("fetch_page: url={}", params.url);

    // Determine fetch strategy based on URL scheme
    if params.url.contains('@') || params.url.starts_with("hvym://") {
        // HVYM datapod URL — resolve via IPFS
        // TODO: Extract CID from datapod URL, fetch via IPFS client
        //   let cid = resolve_datapod_url(&params.url)?;
        //   let data = ipfs.fetch(&cid).await?;

        let result = Result {
            url: params.url,
            content_type: "text/html".into(),
            body: String::new(),
            status: "not_implemented".into(),
        };
        serde_json::to_value(result).map_err(|e| LupusError::Json(e))
    } else {
        // HTTPS URL — standard fetch
        // TODO: HTTP client fetch (reqwest or similar)
        //   let response = client.get(&params.url).send().await?;

        let result = Result {
            url: params.url,
            content_type: "text/html".into(),
            body: String::new(),
            status: "not_implemented".into(),
        };
        serde_json::to_value(result).map_err(|e| LupusError::Json(e))
    }
}
