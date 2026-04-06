//! Local semantic search index — stores document embeddings + metadata,
//! supports nearest‑neighbor queries, persisted between sessions.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::IndexConfig;
use crate::error::LupusError;
use crate::protocol::{ComponentState, IndexInfo};

/// A single document in the search index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub url: String,
    pub title: String,
    pub summary: String,
    pub keywords: Vec<String>,
    /// Embedding vector (dimension depends on model).
    pub embedding: Vec<f32>,
    /// Unix timestamp when this entry was indexed.
    pub indexed_at: u64,
}

pub struct SearchIndex {
    index_path: PathBuf,
    max_entries: usize,
    contribution_mode: String,
    entries: Vec<IndexEntry>,
    last_sync: Option<String>,
}

impl SearchIndex {
    /// Load an existing index from disk, or create a new empty one.
    pub fn load_or_create(config: &IndexConfig) -> Result<Self, LupusError> {
        let index_file = config.path.join("index.json");
        let entries = if index_file.exists() {
            tracing::info!("Loading search index from {}", index_file.display());
            let data = std::fs::read_to_string(&index_file)
                .map_err(|e| LupusError::Index(format!("read: {}", e)))?;
            serde_json::from_str(&data)
                .map_err(|e| LupusError::Index(format!("parse: {}", e)))?
        } else {
            tracing::info!("Creating new search index at {}", config.path.display());
            if !config.path.exists() {
                std::fs::create_dir_all(&config.path)
                    .map_err(|e| LupusError::Index(format!("mkdir: {}", e)))?;
            }
            Vec::new()
        };

        let count = entries.len();
        tracing::info!("Search index ready ({} entries)", count);

        Ok(Self {
            index_path: config.path.clone(),
            max_entries: config.max_entries,
            contribution_mode: config.contribution_mode.clone(),
            entries,
            last_sync: None,
        })
    }

    /// Add a document to the index.
    pub fn add(&mut self, entry: IndexEntry) -> Result<(), LupusError> {
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

    /// Semantic search: find the top‑k entries most similar to the query embedding.
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<&IndexEntry> {
        let mut scored: Vec<(f64, &IndexEntry)> = self
            .entries
            .iter()
            .filter(|e| !e.embedding.is_empty())
            .map(|e| (cosine_similarity(query_embedding, &e.embedding), e))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(top_k).map(|(_, e)| e).collect()
    }

    /// Keyword fallback search (when embeddings aren't available yet).
    pub fn search_keyword(&self, query: &str, top_k: usize) -> Vec<&IndexEntry> {
        let terms: Vec<&str> = query.to_lowercase().leak().split_whitespace().collect();
        let mut scored: Vec<(usize, &IndexEntry)> = self
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

    /// Persist the index to disk.
    pub fn save(&self) -> Result<(), LupusError> {
        let index_file = self.index_path.join("index.json");
        let data = serde_json::to_string(&self.entries)
            .map_err(|e| LupusError::Index(format!("serialize: {}", e)))?;
        std::fs::write(&index_file, data)
            .map_err(|e| LupusError::Index(format!("write: {}", e)))?;
        tracing::debug!("Index saved ({} entries)", self.entries.len());
        Ok(())
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn contribution_mode(&self) -> &str {
        &self.contribution_mode
    }

    pub fn info(&self) -> IndexInfo {
        IndexInfo {
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
