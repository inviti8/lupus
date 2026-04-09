//! `fetch_page` — fetch page content by URL.
//!
//! Delegates to [`crate::host_rpc::fetch`] which sends a `host_fetch`
//! request to the connected Lepus browser. The browser handles
//! `https://`, `http://`, AND `hvym://` URLs transparently — its
//! `HvymProtocolHandler.sys.mjs` registers `hvym` as a real Necko
//! protocol so a single Web `fetch()` call routes both schemes
//! correctly. The daemon side has no scheme-specific code path.
//!
//! See `docs/LUPUS_TOOLS.md` §2 for the architecture decision (Option B,
//! delegate fetching to the browser) and `docs/LEPUS_CONNECTORS.md` §5
//! for the browser-side handler this talks to.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::LupusError;
use crate::host_rpc;
use super::ToolSchema;

#[derive(Debug, Deserialize)]
struct Params {
    url: String,
}

/// Tool-level result shape — what the agent's executor stores in the
/// observation slot for `$N` references downstream. Exposes the most
/// useful subset of the wire-level [`crate::protocol::HostFetchResult`].
#[derive(Debug, Serialize)]
struct PageResult {
    /// Final URL after redirects (the post-fetch canonical URL).
    url: String,
    /// HTTP status code from the upstream server (200, 404, ...).
    /// NOT the daemon-RPC status — a 404 returned here still means
    /// the fetch attempt itself completed without infrastructure error.
    http_status: u16,
    /// Verbatim from the response Content-Type header.
    content_type: String,
    /// Response body as UTF-8. Empty for binary content (per the v0.1
    /// open question 1 in `docs/LEPUS_CONNECTORS.md` §10).
    body: String,
    /// `true` if the body was cut at the 8 MB cap on the browser side.
    truncated: bool,
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

    let fetched = host_rpc::fetch(&params.url).await.map_err(|e| LupusError::ToolError {
        tool: "fetch_page".into(),
        message: e.to_string(),
    })?;

    if fetched.truncated {
        tracing::warn!(
            "fetch_page: body for {} was truncated at 8 MB on browser side",
            params.url
        );
    }

    let result = PageResult {
        url: fetched.final_url,
        http_status: fetched.http_status,
        content_type: fetched.content_type,
        body: fetched.body,
        truncated: fetched.truncated,
    };
    serde_json::to_value(result).map_err(LupusError::Json)
}
