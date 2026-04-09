// Lupus Daemon — Pathfinder AI for Lepus Browser
//
// Runs as a standalone process alongside Lepus. Communicates via
// WebSocket on localhost:9549.
//
// Components:
//   - Agent: TinyAgent-based search (the "hunt" — LLMCompiler agent loop)
//   - Security: Code-trained model for HTML/JS threat analysis
//   - IPFS: Lightweight client (Iroh) for content fetching and indexing
//   - Crawler: Distributed index builder
//   - Den: Local content store + semantic search index (the wolf's den)
//   - Tools: search_subnet, fetch_page, extract_content, scan_security

// All modules live in src/lib.rs (the lupus library crate). main.rs is
// just the binary entry point — it imports types from `lupus::*` and
// wires them up. This split lets examples, sibling binaries, and
// integration tests reach the same modules.

use std::sync::Arc;

use lupus::agent::Agent;
use lupus::config::Config;
use lupus::crawler::Crawler;
use lupus::daemon::Daemon;
use lupus::den::Den;
use lupus::ipfs::IpfsClient;
use lupus::security::SecurityScanner;

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

    // 6. Load or create the den (local content store + search index)
    let den = match Den::load_or_create(&config.den) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to initialize den: {}", e);
            std::process::exit(1);
        }
    };

    // 7. Assemble daemon and start serving
    let daemon = Arc::new(Daemon::new(agent, security, ipfs, crawler, den, config));

    if let Err(e) = lupus::server::run(daemon).await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }
}
