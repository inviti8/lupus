//! IPC protocol types for the Lupus ↔ Lepus WebSocket connection.
//!
//! All messages are JSON over `ws://localhost:9549`.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize)]
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
    pub version: String,
    pub models: ModelStatus,
    pub ipfs: ComponentState,
    pub index: IndexInfo,
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
pub struct IndexInfo {
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
