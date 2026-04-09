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

use crate::agent::Agent;
use crate::config::Config;
use crate::crawler::Crawler;
use crate::den;
use crate::error::LupusError;
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
