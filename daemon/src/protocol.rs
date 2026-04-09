//! IPC protocol types for the Lupus ↔ Lepus WebSocket connection.
//!
//! All messages are JSON over `ws://localhost:9549`.
//!
//! ## v0.1 alpha contract
//!
//! This module is the **canonical wire-format source of truth**. If
//! anything else (docs, the Lepus-side `LupusClient.sys.mjs`, the mock
//! peers) drifts from the types here, the Rust file wins. See
//! `docs/LUPUS_TOOLS.md` §7 for the full hardening contract and
//! `crate::protocol_codes` for the error code vocabulary.
//!
//! ### Versioning rule
//!
//! New fields are additive only. Both halves silently ignore unknown
//! fields. Removing or renaming a field requires bumping
//! [`PROTOCOL_VERSION`].

use serde::{Deserialize, Serialize};

/// The IPC protocol version this daemon speaks. Returned in
/// [`StatusResponse::protocol_version`] so the browser-side
/// `LupusClient` can refuse to connect against an incompatible daemon
/// rather than silently misinterpreting messages.
///
/// Bump this string ONLY for breaking wire-format changes (renamed or
/// removed fields, removed methods, semantic shifts). Additive
/// changes (new fields, new methods, new error codes) keep the same
/// version.
pub const PROTOCOL_VERSION: &str = "0.1";

// ---------------------------------------------------------------------------
// Request / Response envelope
// ---------------------------------------------------------------------------

/// Incoming request from the Lepus browser.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub id: String,
    pub method: String,
    #[serde(default = "serde_json::Value::default")]
    pub params: serde_json::Value,
}

/// Outgoing response to the Lepus browser.
#[derive(Debug, Serialize)]
pub struct Response {
    pub id: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

impl Response {
    pub fn ok(id: impl Into<String>, result: impl Serialize) -> Self {
        Self {
            id: id.into(),
            status: Status::Ok,
            result: Some(serde_json::to_value(result).unwrap_or_default()),
            error: None,
        }
    }

    pub fn error(id: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status: Status::Error,
            result: None,
            error: Some(ErrorPayload {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Method‑specific param / result types
// ---------------------------------------------------------------------------

// -- search -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    /// Natural-language answer from the joinner second pass. Present
    /// when the agent loop completed successfully and the joinner
    /// produced an `Action: Finish(<answer>)` payload. The browser UI
    /// should render this as the primary user-facing reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_answer: Option<String>,

    /// Per-step record of what the planner emitted and what each tool
    /// returned, in plan order. Present whenever the agent loop ran far
    /// enough to produce a plan. The browser UI may render this as a
    /// "chain of thought" view next to the text answer for transparency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<PlanStepRecord>>,

    /// Structured search hits harvested from the executed plan. Empty
    /// when the plan didn't include search tools (e.g. abstention,
    /// fetch-only, security scans). The fetch_page/extract_content
    /// pipeline may also populate this from extract_content's keywords
    /// in a future iteration.
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub summary: String,
    pub trust_score: u8,
    pub commitment: f64,
}

/// Wire-format mirror of `agent::executor::ExecutionRecord` for
/// inclusion in `SearchResponse`. Doesn't expose the internal `PlanStep`
/// type or the `Vec<PlanArg>`-typed args; uses the simpler `raw_args`
/// string and a flat observation/error pair for browser-side rendering.
#[derive(Debug, Serialize)]
pub struct PlanStepRecord {
    /// Step number as emitted by the planner (starts at 1).
    pub idx: u32,
    /// Tool name as emitted by the planner. May be a hallucinated tool
    /// (e.g. `compose_email`) — the dispatcher's safety net would have
    /// returned an error in that case, captured in `error`.
    pub tool: String,
    /// Original arg string from inside the parentheses, including any
    /// `$N` references the planner emitted.
    pub raw_args: String,
    /// The tool's JSON output if execution succeeded. `None` for
    /// `join()` terminators and for steps that errored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<serde_json::Value>,
    /// Human-readable error if the step failed during arg coercion or
    /// tool dispatch. Mutually exclusive with `observation`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True if this step is a `join`/`join_finish`/`join_replan`
    /// terminator. Useful for the browser to render plan terminators
    /// distinctly from real tool calls.
    pub is_join: bool,
}

// -- scan_page --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ScanParams {
    pub html: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct ScanResponse {
    pub score: u8,
    pub threats: Vec<ThreatIndicator>,
}

#[derive(Debug, Serialize)]
pub struct ThreatIndicator {
    pub kind: String,
    pub description: String,
    pub severity: String, // "low" | "medium" | "high" | "critical"
}

// -- summarize --------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SummarizeParams {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub html: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SummarizeResponse {
    pub title: String,
    pub summary: String,
}

// -- index_page -------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct IndexPageParams {
    pub url: String,
    pub html: String,
    #[serde(default)]
    pub title: Option<String>,
}

// -- get_status -------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// IPC protocol version the daemon is speaking. The Lepus-side
    /// `LupusClient` checks this immediately on connect and refuses to
    /// proceed if it doesn't match its known version. See
    /// [`PROTOCOL_VERSION`] and `docs/LUPUS_TOOLS.md` §7.
    pub protocol_version: String,
    /// Daemon binary version (`CARGO_PKG_VERSION`).
    pub version: String,
    pub models: ModelStatus,
    pub ipfs: ComponentState,
    // Field name stays "index" on the wire (it's still an "index" of pages
    // from the user's POV); the Rust type name follows internal naming.
    pub index: DenInfo,
}

#[derive(Debug, Serialize)]
pub struct ModelStatus {
    pub search: ComponentState,
    pub search_adapter: String,
    pub security: ComponentState,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentState {
    Ready,
    Loading,
    Error,
    Disabled,
}

#[derive(Debug, Serialize)]
pub struct DenInfo {
    pub entries: usize,
    pub last_sync: Option<String>,
    pub status: ComponentState,
}

// -- swap_adapter -----------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SwapAdapterParams {
    pub adapter: String,
}

#[derive(Debug, Serialize)]
pub struct SwapAdapterResponse {
    pub adapter: String,
}

// -- index_stats ------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct IndexStatsResponse {
    pub entries: usize,
    pub last_sync: Option<String>,
    pub contribution_mode: String,
}

// ---------------------------------------------------------------------------
// Daemon → Browser direction (host RPC) — see crate::host_rpc
// ---------------------------------------------------------------------------
//
// The daemon can originate requests TO the browser (e.g. "fetch this URL on
// my behalf"). These use the same envelope shape as browser-originated
// messages — only the direction differs. Daemon-originated request ids are
// prefixed `daemon-req-N` to keep them in a separate namespace from the
// browser's `req-N` ids.
//
// See `docs/LUPUS_TOOLS.md` §3 for the architecture decision (Option B,
// delegate fetching to the browser) and `docs/LEPUS_CONNECTORS.md` for the
// browser-side handler that this talks to.

/// Envelope for a daemon-originated request. Identical shape to
/// [`Request`] above — the type exists so the daemon can `serde::Serialize`
/// outbound requests with the right field set, mirroring how browser
/// requests are deserialized via [`Request`].
#[derive(Debug, Serialize)]
pub struct DaemonRequest<P: Serialize> {
    pub id: String,
    pub method: String,
    pub params: P,
}

// -- host_fetch -------------------------------------------------------------

/// Parameters sent FROM the daemon TO the browser to ask for a URL fetch.
/// The browser handles `https://`, `http://`, AND `hvym://` (the
/// HvymProtocolHandler in `browser/components/hvym/HvymProtocolHandler.sys.mjs`
/// makes hvym a real Necko scheme, so the browser-side handler can use a
/// single Web `fetch()` call regardless of scheme).
#[derive(Debug, Serialize)]
pub struct HostFetchParams {
    pub url: String,
    /// HTTP method. Defaults to `"GET"`. Reserved field — the daemon
    /// only ever sends GET today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Extra request headers. Empty by default. Cookies are NOT set by
    /// the daemon — the browser uses its own cookie store via fetch()'s
    /// default `credentials: "include"` behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<serde_json::Value>,
    /// Reserved for POST/PUT bodies. None today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// The result the browser sends back for a `host_fetch` request.
///
/// `http_status` is the HTTP status code (200, 404, 500, ...) — NOT the
/// daemon-RPC status. A 404 from the server is `status: "ok"` at the RPC
/// layer (the fetch attempt completed without infrastructure error) with
/// `http_status: 404` in the result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostFetchResult {
    /// Echoes the requested URL verbatim.
    pub url: String,
    /// URL after redirects. May differ from `url`.
    pub final_url: String,
    /// HTTP status code (200, 404, ...).
    pub http_status: u16,
    /// Verbatim from the response Content-Type header.
    pub content_type: String,
    /// Response body as a UTF-8 string. Binary bodies are returned as
    /// `body: ""` for v0.1 — see `docs/LEPUS_CONNECTORS.md` open
    /// question 1.
    pub body: String,
    /// `true` if the body was cut at the 8 MB cap. The daemon logs a
    /// warning when this fires so we can spot pages that consistently
    /// hit the limit.
    #[serde(default)]
    pub truncated: bool,
    /// Unix timestamp (seconds) when the fetch completed.
    pub fetched_at: u64,
}
