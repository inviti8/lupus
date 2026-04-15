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
    /// User-curation signal: `true` means the user explicitly archived
    /// this page via the `archive_page` IPC method (the pin icon in
    /// the URL bar), `false` means it was added by the background
    /// `index_page` path or the agent's `crawl_index` tool. Pinned
    /// entries are exempt from capacity-driven eviction in
    /// [`Den::add`]. In Phase 5 this flag propagates under the page's
    /// canonical URL as a trust signal over the cooperative gossip
    /// layer. See `docs/LUPUS_TOOLS.md` §4.6.
    #[serde(default)]
    pub pinned: bool,
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

/// Pin an entry to the den (user-curation path — the `archive_page`
/// IPC method). Forces `pinned: true` regardless of what the caller
/// passed, so a pinned entry can never be accidentally demoted by
/// going through this path.
pub async fn pin_page(mut entry: DenEntry) -> Result<(), LupusError> {
    entry.pinned = true;
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
    /// same URL. Evicts the oldest entry when at capacity, preferring
    /// unpinned entries — a pinned entry is only evicted if every
    /// entry in the den is pinned.
    ///
    /// If the incoming entry has the same URL as an existing pinned
    /// entry, the pin carries forward (we take the max of old.pinned
    /// and new.pinned) so a background `index_page` pass doesn't
    /// silently demote a user's curation signal.
    pub fn add(&mut self, mut entry: DenEntry) -> Result<(), LupusError> {
        // Preserve pinned state if an earlier copy was pinned.
        if let Some(existing) = self.entries.iter().find(|e| e.url == entry.url) {
            if existing.pinned {
                entry.pinned = true;
            }
        }
        self.entries.retain(|e| e.url != entry.url);

        // Evict at capacity — prefer unpinned, oldest first.
        if self.entries.len() >= self.max_entries {
            // Find the oldest unpinned entry. If everything is pinned,
            // fall back to the oldest overall (a user who pins more
            // than max_entries still gets churn, but only within their
            // own pinned set).
            let oldest_unpinned_idx = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.pinned)
                .min_by_key(|(_, e)| e.fetched_at)
                .map(|(i, _)| i);

            let evict_idx = match oldest_unpinned_idx {
                Some(i) => i,
                None => self
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, e)| e.fetched_at)
                    .map(|(i, _)| i)
                    .unwrap_or(0),
            };
            self.entries.remove(evict_idx);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(url: &str, fetched_at: u64, pinned: bool) -> DenEntry {
        DenEntry {
            url: url.into(),
            title: String::new(),
            summary: String::new(),
            keywords: vec![],
            content_type: String::new(),
            content_cid: String::new(),
            embedding: vec![],
            fetched_at,
            pinned,
        }
    }

    fn small_den() -> Den {
        Den {
            den_path: PathBuf::from("/tmp/never-written"),
            max_entries: 3,
            contribution_mode: "off".into(),
            entries: vec![],
            last_sync: None,
        }
    }

    #[test]
    fn add_at_capacity_evicts_oldest_unpinned_first() {
        let mut den = small_den();
        // Capacity 3. Fill with two pinned + one unpinned, all distinct urls.
        // The unpinned one should evict on the next add even though it's
        // not the oldest.
        den.add(make_entry("https://a", 100, true)).unwrap();
        den.add(make_entry("https://b", 200, false)).unwrap();
        den.add(make_entry("https://c", 300, true)).unwrap();
        assert_eq!(den.entries.len(), 3);

        // Adding a 4th should evict "https://b" (the only unpinned),
        // even though "https://a" is older.
        den.add(make_entry("https://d", 400, false)).unwrap();
        assert_eq!(den.entries.len(), 3);
        let urls: Vec<&str> = den.entries.iter().map(|e| e.url.as_str()).collect();
        assert!(urls.contains(&"https://a"), "pinned oldest should survive");
        assert!(urls.contains(&"https://c"), "pinned middle should survive");
        assert!(urls.contains(&"https://d"), "new entry should be present");
        assert!(!urls.contains(&"https://b"), "unpinned should be evicted");
    }

    #[test]
    fn add_at_capacity_evicts_pinned_only_when_no_unpinned_remain() {
        let mut den = small_den();
        den.add(make_entry("https://a", 100, true)).unwrap();
        den.add(make_entry("https://b", 200, true)).unwrap();
        den.add(make_entry("https://c", 300, true)).unwrap();

        // Everything is pinned. Adding a 4th must evict the oldest
        // pinned entry — pin-only is the fallback bucket.
        den.add(make_entry("https://d", 400, true)).unwrap();
        let urls: Vec<&str> = den.entries.iter().map(|e| e.url.as_str()).collect();
        assert!(!urls.contains(&"https://a"), "oldest pinned evicted");
        assert!(urls.contains(&"https://d"));
        assert_eq!(den.entries.len(), 3);
    }

    #[test]
    fn re_adding_unpinned_url_preserves_existing_pin() {
        let mut den = small_den();
        den.add(make_entry("https://a", 100, true)).unwrap();
        // A background re-index of the same URL with pinned=false MUST
        // NOT silently demote the user's curation signal.
        den.add(make_entry("https://a", 200, false)).unwrap();
        assert_eq!(den.entries.len(), 1);
        assert!(den.entries[0].pinned, "pin should carry forward");
        assert_eq!(den.entries[0].fetched_at, 200, "metadata refreshed");
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
