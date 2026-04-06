//! `crawl_index` — create an index entry from a CID or URL.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    /// CID or URL of the content to index.
    source: String,
}

#[derive(Debug, Serialize)]
struct Result {
    indexed: bool,
    url: String,
    title: String,
}

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: "crawl_index",
        description: "Fetch content by CID or URL and create a local index entry",
        parameters: json!({
            "type": "object",
            "properties": {
                "source": { "type": "string", "description": "CID or URL to fetch and index" }
            },
            "required": ["source"]
        }),
    }
}

pub async fn execute(args: serde_json::Value) -> std::result::Result<serde_json::Value, LupusError> {
    let params: Params = serde_json::from_value(args)
        .map_err(|e| LupusError::ToolError { tool: "crawl_index".into(), message: e.to_string() })?;

    tracing::debug!("crawl_index: source={}", params.source);

    // TODO: Fetch content (via IPFS if CID, via HTTP if URL),
    //   extract metadata, add to index
    //
    //   let content = if looks_like_cid(&params.source) {
    //       ipfs.fetch(&params.source).await?
    //   } else {
    //       http_fetch(&params.source).await?
    //   };
    //   let entry = build_index_entry(&content);
    //   index.add(entry)?;

    let result = Result {
        indexed: false,
        url: params.source,
        title: String::new(),
    };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}
