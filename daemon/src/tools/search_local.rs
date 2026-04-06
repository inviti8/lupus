//! `search_local_index` — query the local semantic search index.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize { 10 }

#[derive(Debug, Serialize)]
struct Result {
    results: Vec<LocalResult>,
}

#[derive(Debug, Serialize)]
struct LocalResult {
    url: String,
    title: String,
    summary: String,
    score: f64,
}

pub fn schema() -> ToolSchema {
    ToolSchema {
        name: "search_local_index",
        description: "Search the local semantic index for previously visited pages",
        parameters: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "top_k": { "type": "integer", "description": "Max results (default 10)" }
            },
            "required": ["query"]
        }),
    }
}

pub async fn execute(args: serde_json::Value) -> std::result::Result<serde_json::Value, LupusError> {
    let params: Params = serde_json::from_value(args)
        .map_err(|e| LupusError::ToolError { tool: "search_local_index".into(), message: e.to_string() })?;

    tracing::debug!("search_local_index: query={:?} top_k={}", params.query, params.top_k);

    // TODO: Generate embedding for query, then search index
    //   let embedding = model.embed(&params.query)?;
    //   let results = index.search(&embedding, params.top_k);

    // Fallback: keyword search (no embeddings yet)
    let result = Result { results: vec![] };
    serde_json::to_value(result).map_err(|e| LupusError::Json(e))
}
