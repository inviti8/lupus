//! `crawl_index` — fetch a URL and add it to the den.
//!
//! The agent's tool for "go grab this and remember it". Pulls bytes via
//! `host_rpc::fetch` (which the browser handles via Necko, supporting
//! both `https://` and `hvym://` transparently), stores the body in the
//! local Iroh blob store via `crate::ipfs::add_blob`, extracts metadata,
//! and adds an entry to the den (`crate::den::add_page`). The resulting
//! entry carries both the original URL and a content-addressed CID, so
//! later lookups can pull the cached body even if the original URL goes
//! away. See `docs/LUPUS_TOOLS.md` §4.6.
//!
//! ## Source resolution
//!
//! For Phase 3 the `source` parameter is treated as a URL only. The
//! cooperative-CID path (where `source` is a 64-char hex CID and the
//! daemon would fetch from a peer's blob store) is deferred to Phase 5
//! when the gossip layer comes online. Until then, anything that looks
//! like a CID is rejected with a tool error.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::crawler;
use crate::den::{self, DenEntry};
use crate::error::LupusError;
use crate::host_rpc;
use crate::ipfs;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    /// CID or URL of the content to index. For Phase 3 only URLs are
    /// supported (`https://`, `http://`, `hvym://`, or bare
    /// `name@service`). CID sources will land alongside the Phase 5
    /// gossip layer.
    source: String,
}

#[derive(Debug, Serialize)]
struct CrawlResult {
    indexed: bool,
    url: String,
    title: String,
    content_type: String,
    content_cid: String,
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

    if looks_like_cid(&params.source) {
        return Err(LupusError::ToolError {
            tool: "crawl_index".into(),
            message: "CID sources are not supported in Phase 3 (cooperative gossip layer not yet wired — see docs/LUPUS_TOOLS.md §4.4 Phase 5)".into(),
        });
    }

    // Fetch via host_rpc — same delegated path as fetch_page.
    let fetched = host_rpc::fetch(&params.source).await.map_err(|e| LupusError::ToolError {
        tool: "crawl_index".into(),
        message: e.to_string(),
    })?;

    if fetched.truncated {
        tracing::warn!(
            "crawl_index: body for {} was truncated at 8 MB on browser side",
            params.source
        );
    }

    // Extract metadata using the same helpers as the IPC `index_page` path
    // so the two surfaces produce identical entries for identical inputs.
    let title = crawler::extract_title(&fetched.body);
    let summary = crawler::extract_summary(&fetched.body);
    let keywords = crawler::extract_keywords(&fetched.body);

    // Best-effort blob store: if it isn't loaded, log and continue with
    // an empty content_cid (matches the v0.1 contract for the field).
    let content_cid = match ipfs::add_blob(fetched.body.as_bytes()).await {
        Ok(cid) => cid,
        Err(e) => {
            tracing::warn!(
                "crawl_index: blob store unavailable for {}, indexing without content_cid: {}",
                params.source, e
            );
            String::new()
        }
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let entry = DenEntry {
        url: fetched.final_url.clone(),
        title: title.clone(),
        summary,
        keywords,
        content_type: fetched.content_type.clone(),
        content_cid: content_cid.clone(),
        embedding: Vec::new(),
        fetched_at: now,
        pinned: false, // agent-driven path — user pins come via archive_page
    };

    den::add_page(entry)
        .await
        .map_err(|e| LupusError::ToolError {
            tool: "crawl_index".into(),
            message: e.to_string(),
        })?;

    let result = CrawlResult {
        indexed: true,
        url: fetched.final_url,
        title,
        content_type: fetched.content_type,
        content_cid,
    };
    serde_json::to_value(result).map_err(LupusError::Json)
}

/// Heuristic check: does this string look like an iroh-blobs hash?
/// 64 lowercase hex chars or 52 base32-nopad chars. Used to short-
/// circuit the (currently unsupported) CID path.
fn looks_like_cid(s: &str) -> bool {
    let len = s.len();
    if len == 64 {
        s.chars().all(|c| c.is_ascii_hexdigit())
    } else if len == 52 {
        s.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}
