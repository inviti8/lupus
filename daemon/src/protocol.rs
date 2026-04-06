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
