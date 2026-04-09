use std::io;

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
    pub fn code(&self) -> &str {
        match self {
            Self::ModelNotLoaded(_) => "model_not_loaded",
            Self::ModelLoadFailed(_) => "model_load_failed",
            Self::Inference(_) => "inference_error",
            Self::AdapterNotFound(_) => "adapter_not_found",
            Self::InvalidRequest(_) => "invalid_request",
            Self::UnknownMethod(_) => "unknown_method",
            Self::ToolError { .. } => "tool_error",
            Self::Config(_) => "config_error",
            // Wire-level error code stays "index_error" — "index" here is the
            // verb (an error during the indexing operation), not the noun (the
            // storage layer is the den). See docs/LUPUS_TOOLS.md §7.
            Self::Den(_) => "index_error",
            Self::Ipfs(_) => "ipfs_error",
            Self::WebSocket(_) => "websocket_error",
            Self::Io(_) => "io_error",
            Self::Json(_) => "json_error",
            Self::Yaml(_) => "yaml_error",
        }
    }
}
