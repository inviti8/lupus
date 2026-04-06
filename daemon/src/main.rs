// Lupus Daemon — Pathfinder AI for Lepus Browser
//
// Runs as a standalone process alongside Lepus. Communicates via
// WebSocket on localhost:9549.
//
// Components:
//   - Agent: TinyAgent-based search with LoRA adapter hot-swapping
//   - Security: Code-trained model for HTML/JS threat analysis
//   - IPFS: Lightweight client (Iroh) for content fetching and indexing
//   - Crawler: Distributed index builder
//   - Index: Local semantic search with embeddings
//   - Tools: search_subnet, fetch_page, extract_content, scan_security

mod agent;
mod config;
mod crawler;
mod daemon;
mod error;
mod index;
mod ipfs;
mod protocol;
mod security;
mod server;
mod tools;

use std::sync::Arc;

use crate::agent::Agent;
use crate::config::Config;
use crate::crawler::Crawler;
use crate::daemon::Daemon;
use crate::index::SearchIndex;
use crate::ipfs::IpfsClient;
use crate::security::SecurityScanner;

#[tokio::main]
async fn main() {
    // 1. Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Lupus v{} — Pathfinder AI Daemon", env!("CARGO_PKG_VERSION"));

    // 2. Load config
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // 3. Create components
    let mut security = SecurityScanner::new(&config.models);
    let mut agent = Agent::new(&config.models);
    let mut ipfs = IpfsClient::new(&config.ipfs);
    let crawler = Crawler::new();

    // 4. Load models — security first (needed for page scanning on first load)
    tracing::info!("Loading security model...");
    if let Err(e) = security.load().await {
        tracing::warn!("Security model failed to load: {} (heuristic scanning only)", e);
    }

    tracing::info!("Loading search model...");
    if let Err(e) = agent.load().await {
        tracing::warn!("Search model failed to load: {} (search unavailable)", e);
    }

    // 5. Connect IPFS
    if config.ipfs.enabled {
        tracing::info!("Connecting to IPFS gateway...");
        if let Err(e) = ipfs.connect().await {
            tracing::warn!("IPFS connection failed: {} (IPFS features unavailable)", e);
        }
    }

    // 6. Load or create search index
    let index = match SearchIndex::load_or_create(&config.index) {
        Ok(idx) => idx,
        Err(e) => {
            tracing::error!("Failed to initialize search index: {}", e);
            std::process::exit(1);
        }
    };

    // 7. Assemble daemon and start serving
    let daemon = Arc::new(Daemon::new(agent, security, ipfs, crawler, index, config));

    if let Err(e) = server::run(daemon).await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }
}
