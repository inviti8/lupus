//! Lupus daemon configuration — loaded from `~/.config/lupus/config.yaml`
//! (or platform equivalent) with sensible defaults.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::error::LupusError;

// ---------------------------------------------------------------------------
// Top‑level config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub models: ModelsConfig,
    pub ipfs: IpfsConfig,
    pub index: IndexConfig,
    pub cooperative: CooperativeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            models: ModelsConfig::default(),
            ipfs: IpfsConfig::default(),
            index: IndexConfig::default(),
            cooperative: CooperativeConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Section configs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub port: u16,
    pub host: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: 9549,
            host: "127.0.0.1".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub search_base: PathBuf,
    pub search_adapter: PathBuf,
    pub content_adapter: PathBuf,
    pub security: PathBuf,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        let base = data_dir().join("models");
        Self {
            search_base: base.join("lupus-search-base.gguf"),
            search_adapter: base.join("lupus-search-adapter.gguf"),
            content_adapter: base.join("lupus-content-adapter.gguf"),
            // The security model is `Qwen2ForSequenceClassification` loaded
            // via candle-transformers (NOT llama-cpp-2), so this points at
            // the safetensors directory containing config.json,
            // model.safetensors, tokenizer.json — not a single .gguf file.
            security: base.join("lupus-security"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct IpfsConfig {
    pub enabled: bool,
    pub gateway: String,
    pub cache_dir: PathBuf,
    pub max_cache_gb: u64,
}

impl Default for IpfsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gateway: "https://gateway.heavymeta.art".into(),
            cache_dir: data_dir().join("ipfs-cache"),
            max_cache_gb: 5,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    pub path: PathBuf,
    pub max_entries: usize,
    pub contribution_mode: String,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            path: data_dir().join("search-index"),
            max_entries: 100_000,
            contribution_mode: "off".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CooperativeConfig {
    pub registry: String,
    pub contract_id: String,
}

impl Default for CooperativeConfig {
    fn default() -> Self {
        Self {
            registry: "https://registry.heavymeta.art".into(),
            contract_id: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl Config {
    /// Load config from the platform config directory, falling back to defaults.
    pub fn load() -> Result<Self, LupusError> {
        let path = config_path();
        if path.exists() {
            Self::load_from(&path)
        } else {
            tracing::info!("No config file at {}, using defaults", path.display());
            Ok(Self::default())
        }
    }

    /// Load from a specific YAML file.
    pub fn load_from(path: &Path) -> Result<Self, LupusError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| LupusError::Config(format!("{}: {}", path.display(), e)))?;
        let config: Config = serde_yaml::from_str(&contents)?;
        tracing::info!("Loaded config from {}", path.display());
        Ok(config)
    }
}

// ---------------------------------------------------------------------------
// Platform paths
// ---------------------------------------------------------------------------

/// `~/.config/lupus/config.yaml` (Linux) / `%APPDATA%\lupus\config.yaml` (Windows)
fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lupus")
        .join("config.yaml")
}

/// `~/.local/share/lupus/` (Linux) / `%LOCALAPPDATA%\lupus\` (Windows)
fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lupus")
}
