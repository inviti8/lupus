//! The Den — Lupus's local content-addressed store + search index.
//!
//! Named after the wolf's den: where the pack stores what it brings home and
//! returns to look things up. Concretely it holds [`DenEntry`] records for
//! every page Lupus has crawled or been asked to remember (via the
//! `index_page` IPC handler or the `crawl_index` agent tool), and (in
//! Phase 3 onward) also owns the local Iroh blob store that backs each
//! entry's `content_cid`.
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

use serde::{Deserialize, Serialize};

use crate::config::DenConfig;
use crate::error::LupusError;
use crate::protocol::{ComponentState, DenInfo};

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
    /// Embedding vector (dimension depends on model). Empty until the
    /// embedding model lands — see `docs/LUPUS_TOOLS.md` §4.5.
    pub embedding: Vec<f32>,
    /// Unix timestamp when this entry was indexed.
    pub indexed_at: u64,
}

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

    /// Add a document to the den.
    pub fn add(&mut self, entry: DenEntry) -> Result<(), LupusError> {
        // Replace existing entry with same URL
        self.entries.retain(|e| e.url != entry.url);

        // Evict oldest if at capacity
        if self.entries.len() >= self.max_entries {
            self.entries.sort_by_key(|e| e.indexed_at);
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
