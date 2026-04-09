//! `search_subnet` — query the cooperative subnet for matching content.
//!
//! ## Status: DEFERRED to a structured sentinel
//!
//! The cooperative search surface does not exist yet anywhere in the
//! Heavymeta stack. Verified 2026-04-09 across `pintheon_contracts`,
//! `hvym_tunnler`, and `heavymeta_collective` — none of them expose a
//! search API or even a content-metadata schema. See
//! `docs/LUPUS_TOOLS.md` §4.2 for the full investigation.
//!
//! Rather than removing this tool from the dispatch table (which would
//! invalidate the trained planner LoRA — it picks `search_subnet` for
//! ~5/22 eval cases), we keep the tool registered but have it return a
//! structured "not yet built" sentinel. The joinner is taught to
//! recognize `status: "not_implemented"` and produce a graceful "I
//! can't search the cooperative directly yet" message rather than
//! fabricating results from an empty list.
//!
//! ## Re-enable when
//!
//! The cooperative ships either of:
//!   - a search API on `heavymeta.art` over linktree/profile data, or
//!   - an off-chain indexer over `hvym-roster` JOIN events that
//!     fetches each member's IPNS linktree and indexes the contents
//!
//! At that point this file gets a follow-up PR pointing to the new
//! `host_search_cooperative` (or similar) RPC method.

use serde::Deserialize;
use serde_json::json;

use crate::error::LupusError;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    query: String,
    #[serde(default)]
    scope: Option<String>,
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

    // Structured sentinel — see module docs and docs/LUPUS_TOOLS.md §4.2.
    // The tool succeeds at the dispatcher level (no LupusError); the
    // joinner reads `status: "not_implemented"` and routes around it.
    Ok(json!({
        "matches": [],
        "status": "not_implemented",
        "reason": "cooperative search surface not yet built — see docs/LUPUS_TOOLS.md §4.2"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_structured_not_implemented_sentinel() {
        let args = json!({"query": "ceramics handbook", "scope": "hvym"});
        let result = execute(args).await.expect("search_subnet should not error");

        assert_eq!(result["status"], json!("not_implemented"));
        assert_eq!(result["matches"], json!([]));
        assert!(
            result["reason"]
                .as_str()
                .unwrap()
                .contains("cooperative search surface not yet built"),
            "reason should reference the deferral, got: {:?}",
            result["reason"]
        );
    }

    #[tokio::test]
    async fn rejects_missing_query_param() {
        let args = json!({"scope": "hvym"});
        let err = execute(args).await.unwrap_err();
        // Missing required field → ToolError, not a sentinel.
        let msg = format!("{}", err);
        assert!(
            msg.contains("query"),
            "expected error to mention missing query field, got: {msg}"
        );
    }
}
