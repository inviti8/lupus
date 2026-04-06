//! IPFS client via Iroh — lightweight, pure‑Rust IPFS for content
//! fetching, caching, and (opt‑in) index entry publishing.

use std::path::PathBuf;

use crate::config::IpfsConfig;
use crate::error::LupusError;
use crate::protocol::ComponentState;

pub struct IpfsClient {
    gateway: String,
    cache_dir: PathBuf,
    max_cache_bytes: u64,
    enabled: bool,
    state: IpfsState,
}

enum IpfsState {
    Disconnected,
    Connected,
    // TODO: Connected { client: iroh::client::Client }
}

impl IpfsClient {
    pub fn new(config: &IpfsConfig) -> Self {
        Self {
            gateway: config.gateway.clone(),
            cache_dir: config.cache_dir.clone(),
            max_cache_bytes: config.max_cache_gb * 1_073_741_824,
            enabled: config.enabled,
            state: IpfsState::Disconnected,
        }
    }

    /// Connect to the cooperative IPFS gateway.
    pub async fn connect(&mut self) -> Result<(), LupusError> {
        if !self.enabled {
            tracing::info!("IPFS disabled in config");
            return Ok(());
        }

        tracing::info!("Connecting to IPFS gateway: {}", self.gateway);

        // Ensure cache directory exists
        if !self.cache_dir.exists() {
            std::fs::create_dir_all(&self.cache_dir)
                .map_err(|e| LupusError::Ipfs(format!("cache dir: {}", e)))?;
        }

        // TODO: Initialize Iroh client
        //   let client = iroh::client::Client::connect(gateway).await?;

        self.state = IpfsState::Connected;
        tracing::info!("IPFS connected");
        Ok(())
    }

    /// Fetch content by CID. Returns raw bytes. Checks local cache first.
    pub async fn fetch(&self, cid: &str) -> Result<Vec<u8>, LupusError> {
        self.require_connected()?;

        // Check local cache
        let cache_path = self.cache_dir.join(cid);
        if cache_path.exists() {
            tracing::debug!("IPFS cache hit: {}", cid);
            return std::fs::read(&cache_path)
                .map_err(|e| LupusError::Ipfs(format!("cache read: {}", e)));
        }

        tracing::debug!("IPFS fetch: {}", cid);

        // TODO: Fetch via Iroh client
        //   let data = client.get_bytes(cid.parse()?).await?;
        //   std::fs::write(&cache_path, &data)?;  // cache locally
        //   Ok(data)

        Err(LupusError::Ipfs(format!("fetch not yet implemented for CID: {}", cid)))
    }

    /// Publish an index entry to the cooperative (opt‑in).
    pub async fn publish(&self, _key: &str, _data: &[u8]) -> Result<(), LupusError> {
        self.require_connected()?;

        // TODO: Publish via Iroh
        //   client.put_bytes(key, data).await?;

        Ok(())
    }

    pub fn component_state(&self) -> ComponentState {
        if !self.enabled {
            return ComponentState::Disabled;
        }
        match &self.state {
            IpfsState::Connected => ComponentState::Ready,
            IpfsState::Disconnected => ComponentState::Loading,
        }
    }

    fn require_connected(&self) -> Result<(), LupusError> {
        if !self.enabled {
            return Err(LupusError::Ipfs("IPFS disabled".into()));
        }
        match &self.state {
            IpfsState::Connected => Ok(()),
            IpfsState::Disconnected => Err(LupusError::Ipfs("not connected".into())),
        }
    }
}
