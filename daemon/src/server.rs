//! WebSocket server — accepts connections from the Lepus browser, dispatches
//! browser-initiated requests through the Daemon, and routes daemon-initiated
//! requests OUT through the same socket via the host_rpc plumbing.
//!
//! ## Two message directions
//!
//! Each WebSocket connection runs two concurrent tasks:
//!
//! 1. **Reader** (`handle_inbound_loop`) — pulls messages from the socket
//!    and disambiguates by shape:
//!    - `{id, method, params}` → browser-initiated request → handed to
//!      `Daemon::handle_message`, response sent back via the writer
//!    - `{id, status, result?, error?}` with `id` starting `daemon-req-`
//!      → reply to a daemon-initiated request → routed through
//!      `host_rpc::deliver_reply`
//!    - Anything else is logged and dropped.
//!
//! 2. **Writer** (`handle_outbound_loop`) — drains a tokio mpsc that the
//!    rest of the daemon pushes messages onto. The mpsc sender is
//!    registered with `host_rpc::register_sink` so tools anywhere in the
//!    daemon can `host_rpc::fetch(...)` and the request lands on the wire.
//!
//! See `crate::host_rpc` for the daemon-side request originator and
//! `docs/LUPUS_TOOLS.md` §3 for the architectural rationale.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::daemon::Daemon;
use crate::error::LupusError;
use crate::host_rpc::{self, RpcReply};
use crate::protocol::ErrorPayload;

/// Start the WebSocket server. Runs until the process is terminated.
pub async fn run(daemon: Arc<Daemon>) -> Result<(), LupusError> {
    let addr = format!("{}:{}", daemon.config.daemon.host, daemon.config.daemon.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("Lupus listening on ws://{}", addr);

    loop {
        let (stream, peer) = listener.accept().await?;
        let daemon = Arc::clone(&daemon);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(daemon, stream, peer).await {
                tracing::error!("Connection error ({}): {}", peer, e);
            }
        });
    }
}

async fn handle_connection(
    daemon: Arc<Daemon>,
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
) -> Result<(), LupusError> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| LupusError::WebSocket(e.to_string()))?;

    tracing::info!("Client connected: {}", peer);
    let (ws_tx, ws_rx) = ws.split();

    // Outbound channel — anything pushed onto this is written to the
    // socket by the writer task. Registered with host_rpc so tools
    // throughout the daemon can originate daemon→browser requests.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    host_rpc::register_sink(out_tx.clone()).await;

    // Spawn the writer; it owns the WebSocket sink half until it
    // finishes. We keep the reader on this task so the connection
    // lifetime tracks the inbound half (which is what naturally
    // terminates on Close).
    let writer = tokio::spawn(handle_outbound_loop(ws_tx, out_rx, peer));

    // Inbound loop runs to completion (or error) on the calling task.
    let result = handle_inbound_loop(daemon, ws_rx, out_tx, peer).await;

    // Tear down: drop the sink first so the writer's mpsc closes and
    // the writer task exits cleanly.
    host_rpc::unregister_sink().await;
    // Cancel the writer if it's still alive (it should exit on its
    // own once out_tx is dropped, but abort guards against a stuck
    // ws.send).
    writer.abort();
    let _ = writer.await;

    tracing::info!("Client disconnected: {}", peer);
    result
}

async fn handle_inbound_loop(
    daemon: Arc<Daemon>,
    mut ws_rx: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    >,
    out_tx: mpsc::UnboundedSender<String>,
    peer: std::net::SocketAddr,
) -> Result<(), LupusError> {
    while let Some(msg) = ws_rx.next().await {
        let msg = msg.map_err(|e| LupusError::WebSocket(e.to_string()))?;

        match msg {
            Message::Text(text) => {
                // Critical: do NOT await browser→daemon request handlers
                // inline. agent.hunt() can take 30-60s of CPU-bound
                // inference. If we await here, the inbound loop blocks
                // and can't read daemon→browser REPLIES that arrive
                // during the hunt — causing host_fetch deadlocks where
                // the browser's reply sits in the socket buffer while
                // the daemon's 30s timeout fires.
                //
                // Instead: parse the message shape, spawn request
                // handlers as separate tasks (they send their response
                // via out_tx when done), and handle reply messages
                // inline (they're fast — just a HashMap lookup +
                // oneshot send to unblock the pending host_rpc::fetch).
                dispatch_or_spawn(&daemon, &out_tx, text.to_string());
            }
            Message::Close(_) => {
                tracing::debug!("Client {} sent Close", peer);
                break;
            }
            // Ignore binary, ping, pong — tungstenite handles pong automatically
            _ => {}
        }
    }
    Ok(())
}

/// Disambiguate the incoming message by shape and route accordingly.
///
/// - **Browser→daemon requests** (have `method`): spawned as independent
///   tokio tasks so the inbound loop keeps reading. This is the critical
///   fix for the host_fetch deadlock: `agent.hunt()` can take 30-60s of
///   CPU-bound inference, and if we awaited it inline the loop couldn't
///   read the daemon→browser replies that arrive during the hunt.
///
/// - **Daemon→browser replies** (have `status` + `daemon-req-*` id):
///   handled inline because they're fast (HashMap lookup + oneshot send)
///   and MUST be processed ASAP to unblock the pending `host_rpc::fetch`.
///
/// - **Anything else**: logged and dropped.
fn dispatch_or_spawn(
    daemon: &Arc<Daemon>,
    out_tx: &mpsc::UnboundedSender<String>,
    text: String,
) {
    // Cheap shape check: parse to Value so we can inspect fields
    // without committing to a typed deserialization yet.
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("dropping malformed inbound message: {} ({})", e, truncate(&text, 80));
            return;
        }
    };

    if value.get("method").is_some() {
        // Browser → daemon request. SPAWN — do NOT await.
        let daemon = Arc::clone(daemon);
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            let response = daemon.handle_message(&text).await;
            let json = match serde_json::to_string(&response) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("failed to serialize response: {}", e);
                    return;
                }
            };
            if out_tx.send(json).is_err() {
                tracing::warn!("outbound channel closed before response could be sent");
            }
        });
    } else if value.get("status").is_some() {
        // Reply to a daemon-originated request. Handle INLINE — fast
        // path that unblocks a pending host_rpc::fetch oneshot.
        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !host_rpc::is_daemon_request_id(id) {
            tracing::warn!(
                "received reply with non-daemon id {:?} — dropping (browser shouldn't reply to itself)",
                id
            );
            return;
        }
        let reply = parse_reply(&value);
        let id_owned = id.to_string();
        // deliver_reply is async (locks the pending map) but completes
        // in microseconds — spawn a tiny task to keep dispatch_or_spawn
        // synchronous so the inbound loop never blocks.
        tokio::spawn(async move {
            host_rpc::deliver_reply(&id_owned, reply).await;
        });
    } else {
        tracing::warn!(
            "dropping inbound message with neither method nor status: {}",
            truncate(&text, 80)
        );
    }
}

fn parse_reply(value: &serde_json::Value) -> RpcReply {
    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status == "ok" {
        let result = value.get("result").cloned().unwrap_or(serde_json::Value::Null);
        RpcReply::Ok(result)
    } else {
        let err = value.get("error");
        let code = err
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str())
            .unwrap_or("unknown_error")
            .to_string();
        let message = err
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        RpcReply::Err(ErrorPayload { code, message })
    }
}

async fn handle_outbound_loop(
    mut ws_tx: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Message,
    >,
    mut out_rx: mpsc::UnboundedReceiver<String>,
    peer: std::net::SocketAddr,
) {
    while let Some(json) = out_rx.recv().await {
        if let Err(e) = ws_tx.send(Message::Text(json.into())).await {
            tracing::warn!("send to {} failed: {} (closing writer)", peer, e);
            break;
        }
    }
    tracing::debug!("outbound writer for {} exiting", peer);
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}...", &s[..n])
    }
}
