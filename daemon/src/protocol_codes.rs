//! Wire-level error code constants — the v0.1 alpha contract.
//!
//! Single source of truth for every `error.code` string the daemon
//! emits over the IPC. The Lepus side mirrors this list verbatim in
//! `browser/components/lupus/LupusErrorCodes.sys.mjs` so drift between
//! the two halves is grep-detectable.
//!
//! ## Versioning rule
//!
//! New codes can be added freely (additive change). Existing codes
//! must NOT be renamed or removed without bumping
//! [`crate::protocol::PROTOCOL_VERSION`]. See `docs/LUPUS_TOOLS.md` §7
//! for the full hardening contract.

// ── Model lifecycle ─────────────────────────────────────────────────────────

pub const ERR_MODEL_NOT_LOADED:   &str = "model_not_loaded";
pub const ERR_MODEL_LOAD_FAILED:  &str = "model_load_failed";
pub const ERR_INFERENCE:          &str = "inference_error";
pub const ERR_ADAPTER_NOT_FOUND:  &str = "adapter_not_found";

// ── Request / dispatch ──────────────────────────────────────────────────────

pub const ERR_PARSE:              &str = "parse_error";
pub const ERR_INVALID_REQUEST:    &str = "invalid_request";
pub const ERR_UNKNOWN_METHOD:     &str = "unknown_method";

// ── Tools ───────────────────────────────────────────────────────────────────

pub const ERR_TOOL:               &str = "tool_error";
pub const ERR_NOT_IMPLEMENTED:    &str = "not_implemented";

// ── Host fetch (daemon → browser direction) ─────────────────────────────────

pub const ERR_FETCH_FAILED:       &str = "fetch_failed";
pub const ERR_FETCH_TIMEOUT:      &str = "fetch_timeout";
pub const ERR_FETCH_TOO_LARGE:    &str = "fetch_too_large";
pub const ERR_HVYM_UNRESOLVED:    &str = "hvym_unresolved";

// ── Host RPC plumbing ───────────────────────────────────────────────────────

/// No browser is connected, or the connection dropped while a daemon
/// request was in flight.
pub const ERR_HOST_DISCONNECTED:  &str = "host_disconnected";

// ── Den / IPFS ──────────────────────────────────────────────────────────────

/// "index" here is the verb (an error during the indexing operation),
/// not the noun. The storage layer is the den. Wire string preserved
/// for v0.1.
pub const ERR_INDEX:              &str = "index_error";
pub const ERR_IPFS:               &str = "ipfs_error";

// ── Plumbing ────────────────────────────────────────────────────────────────

pub const ERR_CONFIG:             &str = "config_error";
pub const ERR_IO:                 &str = "io_error";
pub const ERR_JSON:               &str = "json_error";
pub const ERR_YAML:               &str = "yaml_error";
pub const ERR_WEBSOCKET:          &str = "websocket_error";
