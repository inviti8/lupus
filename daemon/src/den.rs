//! The Den — Lupus's local content-addressed store + search index.
//!
//! Named after the wolf's den: where the pack stores what it brings home and
//! returns to look things up. Concretely it holds [`DenEntry`] records for
//! every page Lupus has crawled or been asked to remember (via the
//! `index_page` IPC handler or the `crawl_index` agent tool). Each entry
//! carries a `content_cid` pointing into the local Iroh blob store
//! (`crate::ipfs::add_blob`/`get_blob`), making the den a pointer-with-cache
//! over a content-addressed backing store.
//!
//! ## Architecture
//!
//! The `Den` lives in a process-wide lazy global ([`DEN_STATE`]) — same
//! `OnceLock<Mutex<Option<…>>>` pattern as `crate::security::CLASSIFIER`,
//! `crate::host_rpc::HOST_RPC_STATE`, and `crate::ipfs::BLOB_STORE`. Tools,
//! the IPC handlers in `crate::daemon::Daemon`, and the agent's free-function
//! tools all reach the den through the [`add_page`] / [`info`] / etc. free
//! functions without threading state through their signatures.
//!
//! ## Naming convention
//!
//! "Index" is the verb (the action of adding a page to the den), "den" is
//! the noun (the storage). IPC methods that act on the den keep the verb
//! form (`index_page`, `index_stats`) — see `daemon/src/protocol.rs`.
//! Internal Rust types use the noun form. The wire-level error code stays
//! `index_error` for the same reason (an error during indexing, not an
//! error of the den itself).

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::DenConfig;
use crate::error::LupusError;
use crate::protocol::{ComponentState, DenInfo};

// ---------------------------------------------------------------------------
// DenEntry — the wire-level record shape
// ---------------------------------------------------------------------------

/// A single document stored in the den.
///
/// Field set is part of the v0.1 wire contract — see
/// `docs/LUPUS_TOOLS.md` §4.6 and §7. New fields are additive only;
/// removing or renaming a field requires a `PROTOCOL_VERSION` bump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenEntry {
    pub url: String,
    pub title: String,
    pub summary: String,
    pub keywords: Vec<String>,
    /// Verbatim from the response Content-Type header. Empty when the
    /// entry came from a path that didn't capture content type
    /// (legacy crawler entries from before Phase 3).
    #[serde(default)]
    pub content_type: String,
    /// Hex-encoded BLAKE3 CID for the cached HTML body in the local
    /// Iroh blob store. Empty string when the entry was indexed
    /// without storing the body (the legacy `index_page` IPC path
    /// before Phase 3 wired the blob store, or for entries received
    /// over the cooperative gossip layer where we don't yet have the
    /// content locally).
    #[serde(default)]
    pub content_cid: String,
    /// Embedding vector (dimension depends on model). Empty until the
    /// embedding model lands — see `docs/LUPUS_TOOLS.md` §4.5.
    #[serde(default)]
    pub embedding: Vec<f32>,
    /// Unix timestamp (seconds) when the page was fetched.
    pub fetched_at: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Process-wide den. Initialized lazily by [`install`] at daemon
/// startup. Tools and IPC handlers access it via the free functions
/// below.
static DEN_STATE: OnceLock<Mutex<Option<Den>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Den>> {
    DEN_STATE.get_or_init(|| Mutex::new(None))
}

/// Install a loaded `Den` into the global slot. Called once at daemon
/// startup from `main.rs` after `Den::load_or_create`.
pub async fn install(den: Den) {
    let mut guard = slot().lock().await;
    if guard.is_some() {
        tracing::warn!("den already installed — replacing existing instance");
    }
    *guard = Some(den);
    tracing::debug!("den installed in global slot");
}

/// Add an entry to the den. Returns an error if the den isn't loaded.
pub async fn add_page(entry: DenEntry) -> Result<(), LupusError> {
    let mut guard = slot().lock().await;
    let den = guard
        .as_mut()
        .ok_or_else(|| LupusError::Den("den not loaded".into()))?;
    den.add(entry)
}

/// Total entry count. Returns 0 if the den isn't loaded yet (caller
/// shouldn't observe anything other than 0 in that state).
pub async fn entry_count() -> usize {
    let guard = slot().lock().await;
    guard.as_ref().map(|d| d.entry_count()).unwrap_or(0)
}

/// Contribution mode setting (from config). Returns `"off"` if the
/// den isn't loaded.
pub async fn contribution_mode() -> String {
    let guard = slot().lock().await;
    guard
        .as_ref()
        .map(|d| d.contribution_mode().to_string())
        .unwrap_or_else(|| "off".into())
}

/// Component state for the `get_status` IPC handler. Returns `Loading`
/// when the den isn't yet installed (during the brief startup window
/// before `install` runs).
pub async fn info() -> DenInfo {
    let guard = slot().lock().await;
    match guard.as_ref() {
        Some(den) => den.info(),
        None => DenInfo {
            entries: 0,
            last_sync: None,
            status: ComponentState::Loading,
        },
    }
}

/// Persist the den to disk. Called from the shutdown handler. Safe to
/// call when the den isn't loaded (no-op).
pub async fn save() -> Result<(), LupusError> {
    let guard = slot().lock().await;
    if let Some(den) = guard.as_ref() {
        den.save()?;
    }
    Ok(())
}

/// Test-only: clear the global den slot.
#[cfg(test)]
pub async fn reset_for_test() {
    let mut guard = slot().lock().await;
    *guard = None;
}

// ---------------------------------------------------------------------------
// Den — the storage struct, owned by the global slot
// ---------------------------------------------------------------------------

pub struct Den {
    den_path: PathBuf,
    max_entries: usize,
    contribution_mode: String,
    entries: Vec<DenEntry>,
    last_sync: Option<String>,
}

impl Den {
    /// Load an existing den from disk, or create a new empty one.
    pub fn load_or_create(config: &DenConfig) -> Result<Self, LupusError> {
        let den_file = config.path.join("den.json");
        let entries = if den_file.exists() {
            tracing::info!("Loading den from {}", den_file.display());
            let data = std::fs::read_to_string(&den_file)
                .map_err(|e| LupusError::Den(format!("read: {}", e)))?;
            serde_json::from_str(&data)
                .map_err(|e| LupusError::Den(format!("parse: {}", e)))?
        } else {
            tracing::info!("Creating new den at {}", config.path.display());
            if !config.path.exists() {
                std::fs::create_dir_all(&config.path)
                    .map_err(|e| LupusError::Den(format!("mkdir: {}", e)))?;
            }
            Vec::new()
        };

        let count = entries.len();
        tracing::info!("Den ready ({} entries)", count);

        Ok(Self {
            den_path: config.path.clone(),
            max_entries: config.max_entries,
            contribution_mode: config.contribution_mode.clone(),
            entries,
            last_sync: None,
        })
    }

    /// Add a document to the den. Replaces any existing entry with the
    /// same URL. Evicts the oldest entry when at capacity.
    pub fn add(&mut self, entry: DenEntry) -> Result<(), LupusError> {
        // Replace existing entry with same URL
        self.entries.retain(|e| e.url != entry.url);

        // Evict oldest if at capacity
        if self.entries.len() >= self.max_entries {
            self.entries.sort_by_key(|e| e.fetched_at);
            self.entries.remove(0);
        }

        self.entries.push(entry);
        Ok(())
    }

    /// Semantic search: find the top-k entries most similar to the query embedding.
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<&DenEntry> {
        let mut scored: Vec<(f64, &DenEntry)> = self
            .entries
            .iter()
            .filter(|e| !e.embedding.is_empty())
            .map(|e| (cosine_similarity(query_embedding, &e.embedding), e))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(top_k).map(|(_, e)| e).collect()
    }

    /// Keyword fallback search (when embeddings aren't available yet).
    pub fn search_keyword(&self, query: &str, top_k: usize) -> Vec<&DenEntry> {
        let terms: Vec<&str> = query.to_lowercase().leak().split_whitespace().collect();
        let mut scored: Vec<(usize, &DenEntry)> = self
            .entries
            .iter()
            .map(|e| {
                let text = format!("{} {} {}", e.title, e.summary, e.keywords.join(" "))
                    .to_lowercase();
                let hits = terms.iter().filter(|t| text.contains(**t)).count();
                (hits, e)
            })
            .filter(|(hits, _)| *hits > 0)
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(top_k).map(|(_, e)| e).collect()
    }

    /// Persist the den to disk.
    pub fn save(&self) -> Result<(), LupusError> {
        let den_file = self.den_path.join("den.json");
        let data = serde_json::to_string(&self.entries)
            .map_err(|e| LupusError::Den(format!("serialize: {}", e)))?;
        std::fs::write(&den_file, data)
            .map_err(|e| LupusError::Den(format!("write: {}", e)))?;
        tracing::debug!("Den saved ({} entries)", self.entries.len());
        Ok(())
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn contribution_mode(&self) -> &str {
        &self.contribution_mode
    }

    pub fn info(&self) -> DenInfo {
        DenInfo {
            entries: self.entries.len(),
            last_sync: self.last_sync.clone(),
            status: ComponentState::Ready,
        }
    }
}

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
    let mag_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}
