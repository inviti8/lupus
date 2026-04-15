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

    // -- fetch_page tool integration tests ----------------------------------
    //
    // These live alongside the host_rpc tests so they share TEST_MUTEX
    // and the host_rpc global state. Each test installs its own
    // MockHostPeer and routes a real `tools::fetch_page::execute` call
    // through the round-trip.

    #[tokio::test]
    async fn fetch_page_tool_round_trip() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let peer = MockHostPeer::install().await;
        peer.add_fixture(
            "https://example.com/article",
            HostFetchResult {
                url: "https://example.com/article".into(),
                final_url: "https://example.com/article?utm_source=hn".into(),
                http_status: 200,
                content_type: "text/html; charset=utf-8".into(),
                body: "<!doctype html><title>Hi</title><p>body</p>".into(),
                truncated: false,
                fetched_at: 1_744_200_000,
            },
        );
        let handle = peer.spawn();

        let args = serde_json::json!({"url": "https://example.com/article"});
        let result = crate::tools::fetch_page::execute(args)
            .await
            .expect("fetch_page should succeed");

        // Verify the wire-shape the agent's executor will see in
        // observation slots. final_url surfaces as `url` in the tool
        // result so the joinner sees the post-redirect canonical.
        assert_eq!(
            result["url"],
            serde_json::json!("https://example.com/article?utm_source=hn")
        );
        assert_eq!(result["http_status"], serde_json::json!(200));
        assert_eq!(
            result["content_type"],
            serde_json::json!("text/html; charset=utf-8")
        );
        assert!(result["body"]
            .as_str()
            .unwrap()
            .contains("<title>Hi</title>"));
        assert_eq!(result["truncated"], serde_json::json!(false));

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn fetch_page_tool_propagates_host_error() {
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

        let args = serde_json::json!({"url": "https://broken.example.com"});
        let err = crate::tools::fetch_page::execute(args).await.unwrap_err();

        // Should come back as a ToolError carrying the host_fetch
        // code+message in its message string.
        assert_eq!(err.code(), crate::protocol_codes::ERR_TOOL);
        let msg = format!("{}", err);
        assert!(
            msg.contains("fetch_failed") && msg.contains("DNS lookup failed"),
            "expected host_fetch error to propagate, got: {msg}"
        );

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn fetch_page_tool_no_client_returns_tool_error() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        // No mock installed — the host_rpc sink is None.
        let args = serde_json::json!({"url": "https://example.com"});
        let err = crate::tools::fetch_page::execute(args).await.unwrap_err();

        // The disconnected error should surface as a ToolError so the
        // joinner can route around it the same way it handles any
        // other tool failure.
        assert_eq!(err.code(), crate::protocol_codes::ERR_TOOL);
        let msg = format!("{}", err);
        assert!(
            msg.contains("host disconnected") || msg.contains("no browser client"),
            "expected disconnected reason in tool error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn fetch_page_tool_handles_hvym_url() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let peer = MockHostPeer::install().await;
        // The mock + host_rpc::fetch don't care about the scheme —
        // they just pass the URL through to the (simulated) browser,
        // which is what we want. The browser-side HvymProtocolHandler
        // does the real routing.
        peer.add_fixture(
            "hvym://alice@gallery",
            HostFetchResult {
                url: "hvym://alice@gallery".into(),
                final_url: "hvym://alice@gallery".into(),
                http_status: 200,
                content_type: "text/html".into(),
                body: "<h1>Alice's Gallery</h1>".into(),
                truncated: false,
                fetched_at: 1_744_200_000,
            },
        );
        let handle = peer.spawn();

        let args = serde_json::json!({"url": "hvym://alice@gallery"});
        let result = crate::tools::fetch_page::execute(args)
            .await
            .expect("fetch_page should succeed for hvym URLs");

        assert_eq!(result["http_status"], serde_json::json!(200));
        assert!(result["body"]
            .as_str()
            .unwrap()
            .contains("Alice's Gallery"));

        handle.shutdown().await;
    }

    // -- crawl_index tool integration tests ---------------------------------
    //
    // These exercise the full Phase 3 pipeline:
    //   crawl_index -> host_rpc::fetch (mocked) -> ipfs::add_blob -> den::add_page
    //
    // They install both the mock browser peer AND a temporary blob store
    // AND a temporary den, all serialized via TEST_MUTEX so they don't
    // interleave with the other host_rpc tests.

    use crate::config::{DenConfig, IpfsConfig};
    use crate::den::{self, Den};
    use crate::ipfs::{self as ipfs_mod, IpfsClient};
    use std::path::PathBuf;

    fn temp_dir(prefix: &str, tag: &str) -> PathBuf {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("lupus-{prefix}-{pid}-{tag}"));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    /// Spin up a clean den + blob store for a test, returning both the
    /// IpfsClient (which the test holds to keep the FsStore alive) and
    /// the temp paths so the test can clean them up.
    async fn install_full_environment(tag: &str) -> IpfsClient {
        // Reset all globals
        ipfs_mod::reset_for_test().await;
        den::reset_for_test().await;

        // Blob store
        let cache_dir = temp_dir("crawl-blobs", tag);
        let ipfs_cfg = IpfsConfig {
            enabled: true,
            gateway: String::new(),
            cache_dir,
            max_cache_gb: 5,
        };
        let mut client = IpfsClient::new(&ipfs_cfg);
        client.connect().await.expect("blob store should open");

        // Den
        let den_path = temp_dir("crawl-den", tag);
        let den_cfg = DenConfig {
            path: den_path,
            max_entries: 100,
            contribution_mode: "off".into(),
        };
        let den = Den::load_or_create(&den_cfg).expect("den should load");
        den::install(den).await;

        client
    }

    #[tokio::test]
    async fn crawl_index_full_pipeline() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;
        let _client = install_full_environment("full_pipeline").await;

        let peer = MockHostPeer::install().await;
        peer.add_fixture(
            "https://en.wikipedia.org/wiki/Wolf",
            HostFetchResult {
                url: "https://en.wikipedia.org/wiki/Wolf".into(),
                final_url: "https://en.wikipedia.org/wiki/Wolf".into(),
                http_status: 200,
                content_type: "text/html; charset=utf-8".into(),
                body: r#"<!doctype html><html><head><title>Wolf - Wikipedia</title>
<meta name="keywords" content="wolf, canis lupus, mammal"></head>
<body><p>The wolf is a large canine native to Eurasia and North America.</p></body></html>"#
                    .into(),
                truncated: false,
                fetched_at: 1_744_200_000,
            },
        );
        let handle = peer.spawn();

        let args = serde_json::json!({"source": "https://en.wikipedia.org/wiki/Wolf"});
        let result = crate::tools::crawl_index::execute(args)
            .await
            .expect("crawl_index should succeed");

        // Verify the tool result shape
        assert_eq!(result["indexed"], serde_json::json!(true));
        assert_eq!(result["url"], serde_json::json!("https://en.wikipedia.org/wiki/Wolf"));
        assert_eq!(result["title"], serde_json::json!("Wolf - Wikipedia"));
        assert_eq!(
            result["content_type"],
            serde_json::json!("text/html; charset=utf-8")
        );
        let cid = result["content_cid"].as_str().expect("content_cid string");
        assert_eq!(cid.len(), 64, "expected 64-char hex CID, got {cid:?}");

        // Verify the blob is actually retrievable from the store
        let body_bytes = ipfs_mod::get_blob(cid)
            .await
            .expect("get_blob ok")
            .expect("blob should be present after crawl");
        let body_str = std::str::from_utf8(&body_bytes).expect("utf8");
        assert!(body_str.contains("Wolf - Wikipedia"));

        // Verify the den has the entry
        assert_eq!(den::entry_count().await, 1);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn crawl_index_rejects_cid_source() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;
        let _client = install_full_environment("rejects_cid").await;

        // 64 hex chars = looks like a CID, currently unsupported.
        let fake_cid = "0".repeat(64);
        let args = serde_json::json!({"source": fake_cid});
        let err = crate::tools::crawl_index::execute(args)
            .await
            .unwrap_err();
        assert_eq!(err.code(), crate::protocol_codes::ERR_TOOL);
        let msg = format!("{err}");
        assert!(
            msg.contains("CID sources are not supported"),
            "expected CID-deferral message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn archive_page_pins_into_den() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;
        let _client = install_full_environment("archive_pin").await;

        // archive_page goes directly through Daemon::handle_archive_page,
        // not through the host_rpc round-trip — but it still touches the
        // global den + global blob store, so it shares TEST_MUTEX with the
        // other host_rpc tests.

        let html = "<!doctype html><title>Cooperative Bylaws</title><p>Pin me.</p>";
        let entry = crate::den::DenEntry {
            url: "hvym://heavymeta@bylaws".into(),
            title: "Cooperative Bylaws".into(),
            summary: "Pin me.".into(),
            keywords: vec![],
            content_type: "text/html; charset=utf-8".into(),
            content_cid: ipfs_mod::add_blob(html.as_bytes()).await.unwrap(),
            embedding: vec![],
            fetched_at: 1_744_200_000,
            pinned: false, // pin_page must force-set this to true
        };
        crate::den::pin_page(entry).await.expect("pin_page ok");

        assert_eq!(den::entry_count().await, 1);
    }

    #[tokio::test]
    async fn crawl_index_handles_blob_store_unavailable() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;
        // NOTE: only reset/install den, NOT the blob store. Blob store
        // is unloaded for this test.
        ipfs_mod::reset_for_test().await;
        den::reset_for_test().await;
        let den_path = temp_dir("crawl-blob-down", "den");
        let den_cfg = DenConfig {
            path: den_path,
            max_entries: 100,
            contribution_mode: "off".into(),
        };
        let den = Den::load_or_create(&den_cfg).expect("den should load");
        den::install(den).await;

        let peer = MockHostPeer::install().await;
        peer.add_fixture(
            "https://example.com/no-blob-store",
            HostFetchResult {
                url: "https://example.com/no-blob-store".into(),
                final_url: "https://example.com/no-blob-store".into(),
                http_status: 200,
                content_type: "text/html".into(),
                body: "<title>No blob store</title>".into(),
                truncated: false,
                fetched_at: 1_744_200_000,
            },
        );
        let handle = peer.spawn();

        let args = serde_json::json!({"source": "https://example.com/no-blob-store"});
        let result = crate::tools::crawl_index::execute(args)
            .await
            .expect("crawl_index should still succeed without blob store");

        // The entry was indexed, but content_cid is empty (degraded path)
        assert_eq!(result["indexed"], serde_json::json!(true));
        assert_eq!(result["content_cid"], serde_json::json!(""));
        assert_eq!(den::entry_count().await, 1);

        handle.shutdown().await;
    }
}
