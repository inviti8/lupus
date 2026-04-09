//! Distributed crawler / indexer — builds the local search index from
//! pages the user visits. Opt‑in sync with the cooperative index channel.
//!
//! As of Phase 3, `index_page` stores the raw HTML body in the local
//! Iroh blob store via `crate::ipfs::add_blob` (getting back a content
//! CID) before adding the entry to the den. The CID is recorded on the
//! `DenEntry` so future lookups can pull the cached body even if the
//! original URL goes away. See `docs/LUPUS_TOOLS.md` §4.6.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::den::{self, DenEntry};
use crate::error::LupusError;
use crate::ipfs;

pub struct Crawler {
    /// Number of pages indexed this session.
    pages_indexed: u64,
}

impl Crawler {
    pub fn new() -> Self {
        Self { pages_indexed: 0 }
    }

    /// Index a page that the user just visited. Called by the browser
    /// via `index_page` and (in the agent loop) by `crawl_index`.
    /// Stores the body in the local Iroh blob store, extracts metadata,
    /// and adds an entry to the den.
    ///
    /// If the blob store isn't loaded (the daemon was started with
    /// `ipfs.enabled = false` or the FsStore failed to open), the
    /// page is still indexed — `content_cid` is left empty. This
    /// matches the v0.1 contract from `docs/LUPUS_TOOLS.md` §4.6
    /// where the field is documented as "empty string if not yet
    /// stored in Iroh".
    pub async fn index_page(
        &mut self,
        url: &str,
        html: &str,
        title: Option<&str>,
    ) -> Result<(), LupusError> {
        let title = title
            .map(String::from)
            .unwrap_or_else(|| extract_title(html));

        let summary = extract_summary(html);
        let keywords = extract_keywords(html);
        let content_type = "text/html".to_string();

        // Best-effort blob store: if it isn't loaded, log and continue
        // with an empty content_cid. We don't fail the index_page call
        // just because the cache is unavailable.
        let content_cid = match ipfs::add_blob(html.as_bytes()).await {
            Ok(cid) => cid,
            Err(e) => {
                tracing::warn!("blob store unavailable, indexing without content_cid: {}", e);
                String::new()
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = DenEntry {
            url: url.to_string(),
            title,
            summary,
            keywords,
            content_type,
            content_cid,
            embedding: Vec::new(), // TODO: generate embedding via model
            fetched_at: now,
        };

        den::add_page(entry).await?;
        self.pages_indexed += 1;
        tracing::debug!("Indexed page: {} (total this session: {})", url, self.pages_indexed);
        Ok(())
    }

    pub fn pages_indexed(&self) -> u64 {
        self.pages_indexed
    }
}

/// Best‑effort title extraction from raw HTML.
pub(crate) fn extract_title(html: &str) -> String {
    // Look for <title>...</title>
    if let Some(start) = html.find("<title") {
        if let Some(tag_end) = html[start..].find('>') {
            let after = start + tag_end + 1;
            if let Some(close) = html[after..].find("</title>") {
                return html[after..after + close].trim().to_string();
            }
        }
    }
    String::new()
}

/// Extract first meaningful paragraph as summary.
pub(crate) fn extract_summary(html: &str) -> String {
    // Simple: find first <p> content
    if let Some(start) = html.find("<p") {
        if let Some(tag_end) = html[start..].find('>') {
            let after = start + tag_end + 1;
            if let Some(close) = html[after..].find("</p>") {
                let raw = &html[after..after + close];
                // Strip inner tags
                let text = strip_tags(raw);
                if text.len() > 300 {
                    return format!("{}...", &text[..297]);
                }
                return text;
            }
        }
    }
    String::new()
}

/// Extract keywords from meta tags.
pub(crate) fn extract_keywords(html: &str) -> Vec<String> {
    // Look for <meta name="keywords" content="...">
    let lower = html.to_lowercase();
    if let Some(pos) = lower.find("name=\"keywords\"") {
        let region = &html[pos..std::cmp::min(pos + 500, html.len())];
        if let Some(cstart) = region.find("content=\"") {
            let after = cstart + 9;
            if let Some(cend) = region[after..].find('"') {
                return region[after..after + cend]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

/// Naïve tag stripping for summary extraction.
fn strip_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.trim().to_string()
}
