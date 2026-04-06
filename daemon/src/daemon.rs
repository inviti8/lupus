//! Daemon coordinator — holds all components, dispatches IPC messages.

use tokio::sync::RwLock;

use crate::agent::Agent;
use crate::config::Config;
use crate::crawler::Crawler;
use crate::error::LupusError;
use crate::index::SearchIndex;
use crate::ipfs::IpfsClient;
use crate::protocol::*;
use crate::security::SecurityScanner;

/// Central daemon state shared across all WebSocket connections.
pub struct Daemon {
    pub agent: RwLock<Agent>,
    pub security: SecurityScanner,
    pub ipfs: IpfsClient,
    pub crawler: RwLock<Crawler>,
    pub index: RwLock<SearchIndex>,
    pub config: Config,
}

impl Daemon {
    pub fn new(
        agent: Agent,
        security: SecurityScanner,
        ipfs: IpfsClient,
        crawler: Crawler,
        index: SearchIndex,
        config: Config,
    ) -> Self {
        Self {
            agent: RwLock::new(agent),
            security,
            ipfs,
            crawler: RwLock::new(crawler),
            index: RwLock::new(index),
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
        let result = agent.search(params).await?;
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
        let mut index = self.index.write().await;
        crawler.index_page(&mut index, &params.url, &params.html, params.title.as_deref())?;
        Ok(Response::ok(req.id, serde_json::json!({"indexed": true})))
    }

    async fn handle_status(&self, req: Request) -> Result<Response, LupusError> {
        let agent = self.agent.read().await;
        let index = self.index.read().await;
        let result = StatusResponse {
            version: env!("CARGO_PKG_VERSION").into(),
            models: ModelStatus {
                search: agent.component_state(),
                search_adapter: agent.current_adapter().into(),
                security: self.security.component_state(),
            },
            ipfs: self.ipfs.component_state(),
            index: index.info(),
        };
        Ok(Response::ok(req.id, result))
    }

    async fn handle_index_stats(&self, req: Request) -> Result<Response, LupusError> {
        let index = self.index.read().await;
        let result = IndexStatsResponse {
            entries: index.entry_count(),
            last_sync: None,
            contribution_mode: index.contribution_mode().into(),
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

        // Save index state
        let index = self.index.read().await;
        if let Err(e) = index.save() {
            tracing::error!("Failed to save index on shutdown: {}", e);
        }

        // TODO: Close IPFS connections

        Ok(Response::ok(req.id, serde_json::json!({"shutdown": true})))
    }
}
