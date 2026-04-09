//! In-process mock browser peer for `host_rpc` tests.
//!
//! Lets the daemon-side host RPC plumbing be exercised end-to-end
//! without standing up a real Lepus build. Tests install a
//! [`MockHostPeer`], pre-load fixture responses for specific URLs,
//! call `host_rpc::fetch`, and assert on what comes back.
//!
//! ## Lifecycle
//!
//! ```ignore
//! # async fn example() -> anyhow::Result<()> {
//! lupus::host_rpc::reset_for_test().await;
//! let peer = MockHostPeer::install().await;
//! peer.add_fixture("https://example.com", HostFetchResult { ... });
//! let handle = peer.spawn();
//!
//! // Now any host_rpc::fetch("https://example.com") call inside the
//! // daemon will receive the canned response.
//! let result = lupus::host_rpc::fetch("https://example.com").await?;
//! assert_eq!(result.http_status, 200);
//!
//! handle.shutdown().await;
//! # Ok(())
//! # }
//! ```
//!
//! The mock implements only `host_fetch` for v0.1 — adding
//! `host_search_*` later means adding a match arm in
//! [`MockHostPeer::handle_request`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::host_rpc::{deliver_reply, register_sink, RpcReply};
use crate::protocol::{ErrorPayload, HostFetchResult};

/// Canned response for a specific URL. The mock returns the matching
/// fixture from [`MockHostPeer::fixtures`] when `host_rpc::fetch` is
/// called against that URL. Unknown URLs get a default 404 result so
/// tests don't hang.
#[derive(Clone)]
pub enum Fixture {
    Ok(HostFetchResult),
    Err(ErrorPayload),
}

pub struct MockHostPeer {
    fixtures: Arc<Mutex<HashMap<String, Fixture>>>,
    inbound_rx: Option<mpsc::UnboundedReceiver<String>>,
}

impl MockHostPeer {
    /// Install a mock browser peer into the host_rpc global state.
    /// Replaces any previously-registered sink. Tests should call
    /// [`crate::host_rpc::reset_for_test`] first to ensure a clean
    /// slate.
    pub async fn install() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        register_sink(tx).await;
        Self {
            fixtures: Arc::new(Mutex::new(HashMap::new())),
            inbound_rx: Some(rx),
        }
    }

    /// Pre-load a successful fixture response for a URL.
    pub fn add_fixture(&self, url: &str, result: HostFetchResult) {
        self.fixtures
            .lock()
            .unwrap()
            .insert(url.to_string(), Fixture::Ok(result));
    }

    /// Pre-load an error fixture for a URL — useful for testing
    /// `host_fetch` error code propagation.
    pub fn add_error_fixture(&self, url: &str, err: ErrorPayload) {
        self.fixtures
            .lock()
            .unwrap()
            .insert(url.to_string(), Fixture::Err(err));
    }

    /// Spawn a background task that drains the inbound message queue
    /// and dispatches replies. Returns a handle that the test should
    /// `.await` on `shutdown()` for clean teardown.
    pub fn spawn(mut self) -> MockHandle {
        let mut rx = self
            .inbound_rx
            .take()
            .expect("MockHostPeer::spawn called twice");
        let fixtures = Arc::clone(&self.fixtures);
        let handle = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                Self::handle_request(&fixtures, &msg).await;
            }
            tracing::debug!("mock host peer: inbound channel closed, exiting");
        });
        MockHandle { task: handle }
    }

    async fn handle_request(
        fixtures: &Arc<Mutex<HashMap<String, Fixture>>>,
        raw: &str,
    ) {
        // Parse the daemon-req envelope. We don't need the full typed
        // shape here — just id, method, and params.url.
        let envelope: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("mock host peer: malformed inbound: {e} ({raw})");
                return;
            }
        };

        let id = envelope
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let method = envelope.get("method").and_then(|v| v.as_str()).unwrap_or("");

        if method != "host_fetch" {
            tracing::warn!("mock host peer: unsupported method {method}, returning unknown_method");
            deliver_reply(
                &id,
                RpcReply::Err(ErrorPayload {
                    code: "unknown_method".into(),
                    message: format!("mock peer doesn't implement {method}"),
                }),
            )
            .await;
            return;
        }

        let url = envelope
            .pointer("/params/url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let reply = {
            let fixtures = fixtures.lock().unwrap();
            match fixtures.get(&url) {
                Some(Fixture::Ok(result)) => RpcReply::Ok(json!(result)),
                Some(Fixture::Err(err)) => RpcReply::Err(err.clone()),
                None => {
                    // Default: return a 404 so tests against unknown
                    // URLs don't hang on a missing fixture. Tests that
                    // care about specific behavior should add a
                    // fixture.
                    RpcReply::Ok(json!(HostFetchResult {
                        url: url.clone(),
                        final_url: url.clone(),
                        http_status: 404,
                        content_type: "text/plain".into(),
                        body: format!("mock host peer: no fixture for {url}"),
                        truncated: false,
                        fetched_at: 0,
                    }))
                }
            }
        };

        deliver_reply(&id, reply).await;
    }
}

/// Handle for shutting down a spawned [`MockHostPeer`]. Calling
/// [`MockHandle::shutdown`] aborts the background task; the test then
/// continues with a clean global state if it calls
/// [`crate::host_rpc::reset_for_test`] before installing the next
/// mock.
pub struct MockHandle {
    task: JoinHandle<()>,
}

impl MockHandle {
    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

