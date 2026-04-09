//! Daemon → Browser RPC plumbing — the new direction.
//!
//! Lets the daemon originate requests TO the connected browser
//! (currently just `host_fetch`) and receive correlated replies on the
//! same WebSocket. Without this module the daemon can only respond to
//! browser-initiated calls; with it, the agent's tools can ask the
//! browser to do network fetches on the daemon's behalf, which is the
//! load-bearing decision in `docs/LUPUS_TOOLS.md` §2 (Option B —
//! delegate fetching to the browser).
//!
//! ## Architecture
//!
//! - **One global slot** ([`HOST_RPC_STATE`]), same lazy-OnceLock
//!   pattern as `crate::security::CLASSIFIER`. Tools call into this
//!   from anywhere without threading state through their signatures.
//! - **One outbound sink** registered by [`register_sink`] when the
//!   browser connects. Multi-client is intentionally NOT supported in
//!   v0.1 — Lepus is the only consumer, and a single `LupusClient`
//!   shares one WebSocket per browser session.
//! - **Pending requests table** keyed by `daemon-req-N` id. Each entry
//!   holds a `oneshot::Sender` that the [`deliver_reply`] path
//!   completes when the browser's response arrives.
//! - **30 s per-fetch inner timeout** (the agent loop's outer timeout
//!   still applies on top). See `docs/LUPUS_TOOLS.md` §6 question 7.
//!
//! ## Wire format
//!
//! Outgoing request:
//! ```json
//! {"id": "daemon-req-1", "method": "host_fetch", "params": {"url": "..."}}
//! ```
//!
//! Incoming reply (success):
//! ```json
//! {"id": "daemon-req-1", "status": "ok", "result": {...HostFetchResult...}}
//! ```
//!
//! Incoming reply (error):
//! ```json
//! {"id": "daemon-req-1", "status": "error", "error": {"code": "fetch_failed", "message": "..."}}
//! ```
//!
//! See `crate::protocol` for the typed shapes and `docs/LEPUS_CONNECTORS.md`
//! for the browser-side handler that this talks to.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::error::LupusError;
use crate::protocol::{DaemonRequest, ErrorPayload, HostFetchParams, HostFetchResult};

#[cfg(test)]
pub mod mock;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Per-fetch inner timeout. The agent loop's outer timeout still applies
/// on top. Per `docs/LUPUS_TOOLS.md` §6 question 7.
const HOST_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Daemon-originated id prefix. Keeps the daemon's id namespace
/// separated from the browser's `req-N` namespace so reply correlation
/// can never cross-talk. Per `docs/LUPUS_TOOLS.md` §6 question 5.
const DAEMON_REQ_PREFIX: &str = "daemon-req-";

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HOST_RPC_STATE: OnceLock<Mutex<HostRpcState>> = OnceLock::new();

fn state() -> &'static Mutex<HostRpcState> {
    HOST_RPC_STATE.get_or_init(|| Mutex::new(HostRpcState::default()))
}

/// Per-process state for daemon → browser RPC. Single browser client
/// assumed (Lepus connects once per session).
struct HostRpcState {
    /// Outbound message sink — a tokio mpsc that the server.rs writer
    /// task drains and pushes to the WebSocket. `None` when no client
    /// is connected.
    sink: Option<mpsc::UnboundedSender<String>>,
    /// In-flight daemon-originated requests, keyed by `daemon-req-N` id.
    /// When the matching reply arrives, the oneshot is completed and
    /// the entry removed.
    pending: HashMap<String, oneshot::Sender<RpcReply>>,
    /// Monotonic counter for the next daemon-req id.
    next_id: u64,
}

impl Default for HostRpcState {
    fn default() -> Self {
        Self {
            sink: None,
            pending: HashMap::new(),
            next_id: 1,
        }
    }
}

/// Result variant fed into the pending oneshot when a reply arrives.
/// Stays generic over the result payload (raw `serde_json::Value`) so
/// future daemon→browser methods can reuse the same plumbing without
/// changing the type — the caller deserializes into its specific
/// result type after recv.
#[derive(Debug)]
pub enum RpcReply {
    Ok(serde_json::Value),
    Err(ErrorPayload),
}

// ---------------------------------------------------------------------------
// Sink registration (called by server.rs on connect / disconnect)
// ---------------------------------------------------------------------------

/// Register the outbound sink for the currently-connected browser
/// client. Called by `crate::server` when a WebSocket connection is
/// established. Replaces any previous sink — if a second client
/// connects while the first is still around, the second wins
/// (single-client model). Any in-flight requests on the old sink are
/// dropped — they'll time out via [`HOST_FETCH_TIMEOUT`].
pub async fn register_sink(sink: mpsc::UnboundedSender<String>) {
    let mut state = state().lock().await;
    if state.sink.is_some() {
        tracing::warn!("host_rpc: replacing existing browser sink (multi-client not supported in v0.1)");
    }
    state.sink = Some(sink);
    tracing::debug!("host_rpc: sink registered");
}

/// Drop the outbound sink. Called by `crate::server` when the
/// WebSocket disconnects. Pending requests are NOT cancelled here —
/// they'll time out naturally via [`HOST_FETCH_TIMEOUT`] and surface
/// as `ERR_HOST_DISCONNECTED` to their callers.
pub async fn unregister_sink() {
    let mut state = state().lock().await;
    state.sink = None;
    tracing::debug!("host_rpc: sink unregistered");
}

// ---------------------------------------------------------------------------
// Reply delivery (called by server.rs when a daemon-req-* reply arrives)
// ---------------------------------------------------------------------------

/// Returns true if the given id belongs to the daemon's request namespace.
/// Used by `crate::server` to dispatch incoming messages: a reply with
/// a `daemon-req-*` id goes through [`deliver_reply`]; everything else
/// is a browser-initiated request.
pub fn is_daemon_request_id(id: &str) -> bool {
    id.starts_with(DAEMON_REQ_PREFIX)
}

/// Deliver a reply to the matching pending request. Called by
/// `crate::server` when an incoming WebSocket message has a
/// `daemon-req-*` id. Drops the reply silently if no matching pending
/// request exists (orphaned — usually means the request already timed
/// out and got removed).
pub async fn deliver_reply(id: &str, reply: RpcReply) {
    let mut state = state().lock().await;
    if let Some(tx) = state.pending.remove(id) {
        // Drop the lock before send so we don't hold it across the
        // (instantaneous) oneshot send.
        drop(state);
        if tx.send(reply).is_err() {
            tracing::warn!("host_rpc: receiver for {} was dropped before reply arrived", id);
        }
    } else {
        tracing::warn!("host_rpc: orphaned reply for {} (no matching pending request)", id);
    }
}

// ---------------------------------------------------------------------------
// fetch — the public surface that tools call
// ---------------------------------------------------------------------------

/// Ask the connected browser to fetch a URL on the daemon's behalf.
/// Handles `https://`, `http://`, `hvym://`, and bare `name@service`
/// form (the browser's HvymResolver normalizes the bare form).
///
/// Returns the [`HostFetchResult`] from the browser, or an error if:
/// - No browser is connected (`ERR_HOST_DISCONNECTED`)
/// - The fetch failed at the network layer (`ERR_FETCH_FAILED` /
///   `ERR_HVYM_UNRESOLVED` from the browser side)
/// - The 30 s inner timeout fires before a reply arrives
///   (`ERR_FETCH_TIMEOUT`)
pub async fn fetch(url: &str) -> Result<HostFetchResult, LupusError> {
    let params = HostFetchParams {
        url: url.to_string(),
        method: None,
        headers: None,
        body: None,
    };
    let reply = send_request("host_fetch", params).await?;
    match reply {
        RpcReply::Ok(value) => serde_json::from_value(value).map_err(|e| {
            LupusError::HostFetch(format!("decoding host_fetch response: {e}"))
        }),
        RpcReply::Err(err) => Err(LupusError::HostFetch(format!(
            "{}: {}",
            err.code, err.message
        ))),
    }
}

/// Send a daemon-originated request and await its reply. Generic over
/// the params type so we can add `host_search_*` and other methods
/// later without rewriting the plumbing.
async fn send_request<P: Serialize>(method: &str, params: P) -> Result<RpcReply, LupusError> {
    // Allocate id, set up the pending oneshot, push the request to the
    // sink — all under one lock acquisition. We DO NOT await while
    // holding the lock.
    let (id, rx) = {
        let mut state = state().lock().await;

        let sink = state
            .sink
            .as_ref()
            .ok_or_else(|| {
                LupusError::HostDisconnected("no browser client connected".into())
            })?
            .clone();

        let id = format!("{}{}", DAEMON_REQ_PREFIX, state.next_id);
        state.next_id += 1;

        let (tx, rx) = oneshot::channel();
        state.pending.insert(id.clone(), tx);

        let envelope = DaemonRequest {
            id: id.clone(),
            method: method.to_string(),
            params,
        };
        let json = serde_json::to_string(&envelope).map_err(|e| {
            LupusError::HostFetch(format!("serializing {} request: {e}", method))
        })?;

        if sink.send(json).is_err() {
            // The writer task is gone — the sink is dead. Remove the
            // pending entry we just inserted so it doesn't leak.
            state.pending.remove(&id);
            return Err(LupusError::HostDisconnected(
                "browser sink closed before send".into(),
            ));
        }

        (id, rx)
    };

    // Now await the reply with a timeout. If it fires we have to
    // remove the pending entry to prevent the table from leaking.
    match tokio::time::timeout(HOST_FETCH_TIMEOUT, rx).await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(_)) => {
            // The oneshot Sender was dropped without being completed.
            // This shouldn't happen unless deliver_reply has a bug.
            Err(LupusError::HostFetch(format!(
                "reply channel for {id} closed unexpectedly"
            )))
        }
        Err(_elapsed) => {
            // Timeout — drop the pending entry to prevent leak.
            let mut state = state().lock().await;
            state.pending.remove(&id);
            Err(LupusError::HostFetch(format!(
                "host_fetch timed out after {}s ({id})",
                HOST_FETCH_TIMEOUT.as_secs()
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
pub async fn pending_count() -> usize {
    state().lock().await.pending.len()
}

#[cfg(test)]
pub async fn reset_for_test() {
    let mut state = state().lock().await;
    state.sink = None;
    state.pending.clear();
    state.next_id = 1;
}

// ---------------------------------------------------------------------------
// Smoke tests — round-trip the mock peer through the public surface
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::mock::MockHostPeer;
    use super::*;
    use crate::protocol::HostFetchResult;

    /// Tests share process-wide HOST_RPC_STATE so they must run
    /// serially. This mutex serializes them at the test boundary.
    static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn fixture_ok(url: &str, body: &str) -> HostFetchResult {
        HostFetchResult {
            url: url.to_string(),
            final_url: url.to_string(),
            http_status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: body.to_string(),
            truncated: false,
            fetched_at: 1_744_200_000,
        }
    }

    #[tokio::test]
    async fn happy_path_round_trip() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let peer = MockHostPeer::install().await;
        peer.add_fixture(
            "https://example.com",
            fixture_ok("https://example.com", "<!doctype html><h1>hi</h1>"),
        );
        let handle = peer.spawn();

        let result = fetch("https://example.com").await.expect("fetch ok");
        assert_eq!(result.http_status, 200);
        assert_eq!(result.url, "https://example.com");
        assert!(result.body.contains("<h1>hi</h1>"));
        assert!(!result.truncated);

        // Pending table should be empty after a successful round trip.
        assert_eq!(pending_count().await, 0);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn error_fixture_propagates_as_host_fetch_error() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let peer = MockHostPeer::install().await;
        peer.add_error_fixture(
            "https://broken.example.com",
            ErrorPayload {
                code: "fetch_failed".into(),
                message: "DNS lookup failed".into(),
            },
        );
        let handle = peer.spawn();

        let err = fetch("https://broken.example.com").await.unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("fetch_failed") && msg.contains("DNS lookup failed"),
            "expected error to carry code and message, got: {msg}"
        );
        assert_eq!(pending_count().await, 0);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn no_sink_returns_host_disconnected() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        // Note: no MockHostPeer installed — the sink is None.
        let err = fetch("https://example.com").await.unwrap_err();
        assert_eq!(err.code(), crate::protocol_codes::ERR_HOST_DISCONNECTED);
        // Failed-to-send means the pending entry should also have
        // been cleaned up.
        assert_eq!(pending_count().await, 0);
    }

    #[tokio::test]
    async fn missing_fixture_returns_default_404() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let peer = MockHostPeer::install().await;
        // No fixture added — the mock returns its built-in default.
        let handle = peer.spawn();

        let result = fetch("https://unknown.example.com").await.expect("fetch ok");
        assert_eq!(result.http_status, 404);
        assert!(result.body.contains("no fixture"));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn id_namespace_recognition() {
        // Pure unit check on the namespace predicate — no peer needed.
        assert!(is_daemon_request_id("daemon-req-1"));
        assert!(is_daemon_request_id("daemon-req-99999"));
        assert!(!is_daemon_request_id("req-1"));
        assert!(!is_daemon_request_id(""));
        assert!(!is_daemon_request_id("daemonreq-1"));
    }
}
