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
mod security;
mod ipfs;
mod crawler;
mod index;
mod tools;

fn main() {
    println!("Lupus v0.1.0 — Pathfinder AI Daemon");
    println!("Listening on ws://127.0.0.1:9549");

    // TODO: Initialize components and start WebSocket server
    // 1. Load security model (needed immediately for page scanning)
    // 2. Load search model base + default adapter
    // 3. Open IPFS client connection
    // 4. Load or create local search index
    // 5. Start WebSocket server
    // 6. Enter event loop: receive requests, dispatch to components
}
