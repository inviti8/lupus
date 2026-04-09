use std::io;

use crate::protocol_codes::*;

/// Errors produced by the Lupus daemon.
#[derive(Debug, thiserror::Error)]
pub enum LupusError {
    #[error("model not loaded: {0}")]
    ModelNotLoaded(String),

    #[error("model load failed: {0}")]
    ModelLoadFailed(String),

    #[error("inference error: {0}")]
    Inference(String),

    #[error("adapter not found: {0}")]
    AdapterNotFound(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("unknown method: {0}")]
    UnknownMethod(String),

    #[error("tool error [{tool}]: {message}")]
    ToolError { tool: String, message: String },

    #[error("config error: {0}")]
    Config(String),

    #[error("den error: {0}")]
    Den(String),

    #[error("ipfs error: {0}")]
    Ipfs(String),

    #[error("host fetch failed: {0}")]
    HostFetch(String),

    #[error("host disconnected: {0}")]
    HostDisconnected(String),

    #[error("websocket error: {0}")]
    WebSocket(String),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
}

impl LupusError {
    /// Return a short error code suitable for the IPC error response.
    /// All codes come from `crate::protocol_codes` — that module is the
    /// single source of truth for the v0.1 wire vocabulary.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ModelNotLoaded(_) => ERR_MODEL_NOT_LOADED,
            Self::ModelLoadFailed(_) => ERR_MODEL_LOAD_FAILED,
            Self::Inference(_) => ERR_INFERENCE,
            Self::AdapterNotFound(_) => ERR_ADAPTER_NOT_FOUND,
            Self::InvalidRequest(_) => ERR_INVALID_REQUEST,
            Self::UnknownMethod(_) => ERR_UNKNOWN_METHOD,
            Self::ToolError { .. } => ERR_TOOL,
            Self::Config(_) => ERR_CONFIG,
            // Wire-level "index_error" — "index" here is the verb (an error
            // during indexing), not the noun (the storage layer is the den).
            Self::Den(_) => ERR_INDEX,
            Self::Ipfs(_) => ERR_IPFS,
            Self::HostFetch(_) => ERR_FETCH_FAILED,
            Self::HostDisconnected(_) => ERR_HOST_DISCONNECTED,
            Self::WebSocket(_) => ERR_WEBSOCKET,
            Self::Io(_) => ERR_IO,
            Self::Json(_) => ERR_JSON,
            Self::Yaml(_) => ERR_YAML,
        }
    }
}
