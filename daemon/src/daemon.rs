//! Daemon coordinator — holds long-lived components and dispatches IPC
//! messages to the appropriate handlers.
//!
//! As of Phase 3, the den (`crate::den`) and the blob store (`crate::ipfs`)
//! both live in process-wide lazy globals rather than fields on this
//! struct. The handlers below use the free functions from those modules
//! so the agent's free-function tools see exactly the same surface.
//! `Daemon` still owns the things that need exclusive `&mut self` access
//! (the agent + crawler) or that don't fit the global pattern (the
//! security scanner facade).

use tokio::sync::RwLock;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::Agent;
use crate::config::Config;
use crate::crawler::{self, Crawler};
use crate::den::{self, DenEntry};
use crate::error::LupusError;
use crate::ipfs;
use crate::ipfs::IpfsClient;
use crate::protocol::*;
use crate::security::SecurityScanner;

/// Central daemon state shared across all WebSocket connections.
pub struct Daemon {
    pub agent: RwLock<Agent>,
    pub security: SecurityScanner,
    pub ipfs: IpfsClient,
    pub crawler: RwLock<Crawler>,
    pub config: Config,
}

impl Daemon {
    pub fn new(
        agent: Agent,
        security: SecurityScanner,
        ipfs: IpfsClient,
        crawler: Crawler,
        config: Config,
    ) -> Self {
        Self {
            agent: RwLock::new(agent),
            security,
            ipfs,
            crawler: RwLock::new(crawler),
            config,
        }
    }

    /// Dispatch an incoming IPC message to the appropriate handler.
    pub async fn handle_message(&self, raw: &str) -> Response {
        let request: Request = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                return Response::error("unknown", "parse_error", e.to_string());
            }
        };

        let id = request.id.clone();
        match self.dispatch(request).await {
            Ok(resp) => resp,
            Err(e) => Response::error(id, e.code(), e.to_string()),
        }
    }

    async fn dispatch(&self, req: Request) -> Result<Response, LupusError> {
        match req.method.as_str() {
            "search" => self.handle_search(req).await,
            "scan_page" => self.handle_scan(req).await,
            "summarize" => self.handle_summarize(req).await,
            "index_page" => self.handle_index_page(req).await,
            "archive_page" => self.handle_archive_page(req).await,
            "get_status" => self.handle_status(req).await,
            "index_stats" => self.handle_index_stats(req).await,
            "swap_adapter" => self.handle_swap_adapter(req).await,
            "shutdown" => self.handle_shutdown(req).await,
            other => Err(LupusError::UnknownMethod(other.into())),
        }
    }

    // -- Method handlers ----------------------------------------------------

    async fn handle_search(&self, req: Request) -> Result<Response, LupusError> {
        let params: SearchParams = serde_json::from_value(req.params)?;
        let agent = self.agent.read().await;
        let result = agent.hunt(params).await?;
        Ok(Response::ok(req.id, result))
    }

    async fn handle_scan(&self, req: Request) -> Result<Response, LupusError> {
        let params: ScanParams = serde_json::from_value(req.params)?;
        let result = self.security.scan(params).await?;
        Ok(Response::ok(req.id, result))
    }

    async fn handle_summarize(&self, req: Request) -> Result<Response, LupusError> {
        let params: SummarizeParams = serde_json::from_value(req.params)?;
        let agent = self.agent.read().await;
        let result = agent.summarize(params).await?;
        Ok(Response::ok(req.id, result))
    }

    async fn handle_index_page(&self, req: Request) -> Result<Response, LupusError> {
        let params: IndexPageParams = serde_json::from_value(req.params)?;
        let mut crawler = self.crawler.write().await;
        crawler
            .index_page(&params.url, &params.html, params.title.as_deref())
            .await?;
        Ok(Response::ok(req.id, serde_json::json!({"indexed": true})))
    }

    /// User-intent archive path — the pin icon in Lepus's URL bar.
    /// Mirrors `handle_index_page`'s data flow (metadata extraction +
    /// blob store + den) but routes through `den::pin_page` so the
    /// resulting entry is marked `pinned: true` and becomes exempt
    /// from capacity-driven eviction. See `docs/LUPUS_TOOLS.md` §4.6
    /// and the Lepus-side plan at `lepus/docs/LUPUS_INTEGRATION.md` §11.
    ///
    /// The URL is stored verbatim — Lepus has already canonicalized
    /// HVYM URLs to the `hvym://name@service` form so Phase 5 gossip
    /// propagates the curation signal under subnet identity, not
    /// under the ephemeral tunnel URL.
    async fn handle_archive_page(&self, req: Request) -> Result<Response, LupusError> {
        let params: ArchivePageParams = serde_json::from_value(req.params)?;

        // Best-effort blob store: degrade to empty content_cid if the
        // store isn't loaded (matches the v0.1 contract for the field
        // and the behavior of index_page / crawl_index).
        let content_cid = match ipfs::add_blob(params.html.as_bytes()).await {
            Ok(cid) => cid,
            Err(e) => {
                tracing::warn!(
                    "archive_page: blob store unavailable for {}, pinning without content_cid: {}",
                    params.url, e
                );
                String::new()
            }
        };

        let title = if params.title.is_empty() {
            crawler::extract_title(&params.html)
        } else {
            params.title
        };
        let summary = crawler::extract_summary(&params.html);
        let keywords = crawler::extract_keywords(&params.html);
        let content_type = params
            .content_type
            .unwrap_or_else(|| "text/html".to_string());

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = DenEntry {
            url: params.url.clone(),
            title,
            summary,
            keywords,
            content_type,
            content_cid: content_cid.clone(),
            embedding: Vec::new(),
            fetched_at: now,
            pinned: true,
        };

        den::pin_page(entry).await?;
        tracing::info!("archive_page: pinned {}", params.url);

        let result = ArchivePageResponse {
            archived: true,
            content_cid,
        };
        Ok(Response::ok(req.id, result))
    }

    async fn handle_status(&self, req: Request) -> Result<Response, LupusError> {
        let agent = self.agent.read().await;
        let result = StatusResponse {
            protocol_version: PROTOCOL_VERSION.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            models: ModelStatus {
                search: agent.component_state(),
                search_adapter: agent.current_adapter().into(),
                security: self.security.component_state(),
            },
            ipfs: self.ipfs.component_state(),
            index: den::info().await,
        };
        Ok(Response::ok(req.id, result))
    }

    async fn handle_index_stats(&self, req: Request) -> Result<Response, LupusError> {
        let result = IndexStatsResponse {
            entries: den::entry_count().await,
            last_sync: None,
            contribution_mode: den::contribution_mode().await,
        };
        Ok(Response::ok(req.id, result))
    }

    async fn handle_swap_adapter(&self, req: Request) -> Result<Response, LupusError> {
        let params: SwapAdapterParams = serde_json::from_value(req.params)?;
        let mut agent = self.agent.write().await;
        agent.swap_adapter(&params.adapter).await?;
        let result = SwapAdapterResponse {
            adapter: params.adapter,
        };
        Ok(Response::ok(req.id, result))
    }

    async fn handle_shutdown(&self, req: Request) -> Result<Response, LupusError> {
        tracing::info!("Shutdown requested");

        if let Err(e) = den::save().await {
            tracing::error!("Failed to save den on shutdown: {}", e);
        }
        if let Err(e) = self.ipfs.shutdown().await {
            tracing::error!("Failed to shut down blob store: {}", e);
        }

        Ok(Response::ok(req.id, serde_json::json!({"shutdown": true})))
    }
}
