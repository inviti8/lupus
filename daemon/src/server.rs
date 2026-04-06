//! WebSocket server — accepts connections from the Lepus browser and
//! dispatches messages through the Daemon.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::daemon::Daemon;
use crate::error::LupusError;

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
    let (mut tx, mut rx) = ws.split();

    while let Some(msg) = rx.next().await {
        let msg = msg.map_err(|e| LupusError::WebSocket(e.to_string()))?;

        match msg {
            Message::Text(text) => {
                let response = daemon.handle_message(&text).await;
                let json = serde_json::to_string(&response)?;
                tx.send(Message::Text(json.into()))
                    .await
                    .map_err(|e| LupusError::WebSocket(e.to_string()))?;
            }
            Message::Close(_) => {
                tracing::info!("Client disconnected: {}", peer);
                break;
            }
            // Ignore binary, ping, pong — tungstenite handles pong automatically
            _ => {}
        }
    }

    Ok(())
}
