//! IPFS layer — content-addressed local blob store via `iroh-blobs`.
//!
//! Phase 3 (this file): **local-only blob store, no networking.** The
//! daemon uses `iroh_blobs::store::fs::FsStore` to persist crawled HTML
//! bodies (and any other blob the agent decides to remember) under
//! `IpfsConfig::cache_dir`. No sockets are opened, no peers are
//! discovered, no gossip mesh is joined. The store is fully self-
//! contained even with zero connectivity.
//!
//! Phase 5 will wrap the same `FsStore` instance with an
//! `iroh::Endpoint` + `iroh::protocol::Router` to enable cooperative
//! sharing. The on-disk format and `Hash` identifiers are stable
//! across the transition — no data migration required.
//!
//! ## Architecture
//!
//! The `FsStore` lives in a process-wide lazy global
//! ([`BLOB_STORE`]) — same lazy `OnceLock<Mutex<Option<…>>>` pattern
//! as `crate::security::CLASSIFIER` and `crate::host_rpc::HOST_RPC_STATE`.
//! Tools and crawlers reach the blob store through free functions
//! ([`add_blob`], [`get_blob`]) without threading state through their
//! signatures.
//!
//! [`IpfsClient`] is kept as a thin facade owned by `crate::daemon::Daemon`
//! so the existing component-status / config-plumbing surface area
//! doesn't need to change. All real work happens through the global.
//!
//! ## Garbage collection — deferred to Phase 5+
//!
//! `iroh-blobs` has tag-based mark-and-sweep GC, NOT size-based LRU.
//! Since [`add_blob`] auto-creates a persistent tag for each new blob,
//! nothing gets evicted automatically. For Phase 3 we accept unbounded
//! growth — at typical web-page sizes (100–500 KB) the 5 GB default
//! cap from [`crate::config::IpfsConfig::max_cache_gb`] holds 10–50k
//! cached pages, well beyond what a single alpha user will hit.
//!
//! A sidecar size tracker + LRU eviction loop is planned for the
//! Phase 5 work where the cooperative gossip layer comes online and
//! size pressure becomes a real concern. See `docs/LUPUS_TOOLS.md` §4.4.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;

use bytes::Bytes;
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::Hash;
use tokio::sync::Mutex;

use crate::config::IpfsConfig;
use crate::error::LupusError;
use crate::protocol::ComponentState;

// ---------------------------------------------------------------------------
// Global blob store
// ---------------------------------------------------------------------------

/// Process-wide content-addressed blob store. Initialized lazily by
/// [`IpfsClient::connect`] at daemon startup. Tools access it via
/// [`add_blob`] / [`get_blob`].
static BLOB_STORE: OnceLock<Mutex<Option<FsStore>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<FsStore>> {
    BLOB_STORE.get_or_init(|| Mutex::new(None))
}

/// Add `bytes` to the local blob store and return the canonical
/// 64-character lowercase hex CID. Auto-creates a persistent tag so
/// the blob survives Iroh's tag-based GC indefinitely.
///
/// Errors with `LupusError::Ipfs` if the store isn't loaded (the
/// daemon was started with `ipfs.enabled = false` or
/// `IpfsClient::connect` failed) or if the underlying iroh-blobs call
/// returns an error.
pub async fn add_blob(bytes: &[u8]) -> Result<String, LupusError> {
    let guard = slot().lock().await;
    let store = guard
        .as_ref()
        .ok_or_else(|| LupusError::Ipfs("blob store not loaded".into()))?;
    let tag_info = store
        .blobs()
        .add_bytes(Bytes::copy_from_slice(bytes))
        .await
        .map_err(|e| LupusError::Ipfs(format!("add_bytes: {e:?}")))?;
    Ok(tag_info.hash.to_string())
}

/// Look up a blob by its hex CID. Returns `Ok(None)` if the blob isn't
/// in the local store (no fall-through to networking in Phase 3 — Phase
/// 5 will add a remote-peer hop here).
pub async fn get_blob(cid: &str) -> Result<Option<Bytes>, LupusError> {
    // Pre-validate length before handing to iroh-blobs' Hash::from_str.
    // In iroh-blobs 0.99, from_str calls into `data-encoding` without
    // sanitizing the input first — pass it an arbitrary string and the
    // encoding library debug-asserts (panics) instead of returning a
    // clean error. Hash strings are deterministic length: 64 hex chars
    // or 52 base32-nopad chars per `iroh_blobs::Hash::Display`/`FromStr`.
    if cid.len() != 64 && cid.len() != 52 {
        return Err(LupusError::Ipfs(format!(
            "invalid CID {cid:?}: expected 64 hex or 52 base32 chars, got {}",
            cid.len()
        )));
    }
    let hash =
        Hash::from_str(cid).map_err(|e| LupusError::Ipfs(format!("invalid CID {cid:?}: {e}")))?;

    let guard = slot().lock().await;
    let store = guard
        .as_ref()
        .ok_or_else(|| LupusError::Ipfs("blob store not loaded".into()))?;

    // Check presence first so we can distinguish "not in store" from a
    // real I/O error. status() is cheap (metadata-only).
    let status = store
        .blobs()
        .status(hash)
        .await
        .map_err(|e| LupusError::Ipfs(format!("blob status for {cid}: {e:?}")))?;
    if matches!(status, BlobStatus::NotFound) {
        return Ok(None);
    }

    let bytes = store
        .blobs()
        .get_bytes(hash)
        .await
        .map_err(|e| LupusError::Ipfs(format!("get_bytes for {cid}: {e:?}")))?;
    Ok(Some(bytes))
}

/// Test-only: clear the global blob store. Used by unit tests to
/// guarantee a clean slate.
#[cfg(test)]
pub async fn reset_for_test() {
    let mut guard = slot().lock().await;
    if let Some(store) = guard.take() {
        let _ = store.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// IpfsClient — thin facade owned by `Daemon`
// ---------------------------------------------------------------------------

/// Thin facade kept on `crate::daemon::Daemon` so the existing
/// component-status / config plumbing doesn't need to change. All
/// blob operations go through the global [`BLOB_STORE`] via the free
/// functions above.
pub struct IpfsClient {
    cache_dir: PathBuf,
    /// Maximum cache size in bytes. Currently informational only — see
    /// the GC discussion in the module docs. Kept on the struct so the
    /// config field doesn't get dropped from `IpfsConfig`.
    #[allow(dead_code)]
    max_cache_bytes: u64,
    enabled: bool,
}

impl IpfsClient {
    pub fn new(config: &IpfsConfig) -> Self {
        Self {
            cache_dir: config.cache_dir.clone(),
            max_cache_bytes: config.max_cache_gb * 1_073_741_824,
            enabled: config.enabled,
        }
    }

    /// Open the local `FsStore` under `cache_dir` and install it in the
    /// global slot. Idempotent at the per-process level — if the slot
    /// is already populated, this is a warning + no-op. NO sockets are
    /// opened; this is a pure-disk operation.
    pub async fn connect(&mut self) -> Result<(), LupusError> {
        if !self.enabled {
            tracing::info!("IPFS disabled in config — blob store will be unavailable");
            return Ok(());
        }

        tracing::info!(
            "Opening local blob store at {} (no networking)",
            self.cache_dir.display()
        );

        if !self.cache_dir.exists() {
            std::fs::create_dir_all(&self.cache_dir)
                .map_err(|e| LupusError::Ipfs(format!("cache dir: {e}")))?;
        }

        let mut guard = slot().lock().await;
        if guard.is_some() {
            tracing::warn!("blob store already loaded — skipping re-load");
            return Ok(());
        }

        let store = FsStore::load(&self.cache_dir)
            .await
            .map_err(|e| LupusError::Ipfs(format!("FsStore::load: {e:?}")))?;
        *guard = Some(store);

        tracing::info!("Blob store loaded ({})", self.cache_dir.display());
        Ok(())
    }

    /// Status used by the `get_status` IPC handler. Synchronous —
    /// reads the global slot via `try_lock` so it never blocks.
    pub fn component_state(&self) -> ComponentState {
        if !self.enabled {
            return ComponentState::Disabled;
        }
        match slot().try_lock() {
            Ok(guard) => {
                if guard.is_some() {
                    ComponentState::Ready
                } else {
                    ComponentState::Loading
                }
            }
            // Contention → assume still loading
            Err(_) => ComponentState::Loading,
        }
    }

    /// Flush the blob store on shutdown. Safe to call when the store
    /// isn't loaded (no-op).
    pub async fn shutdown(&self) -> Result<(), LupusError> {
        let mut guard = slot().lock().await;
        if let Some(store) = guard.take() {
            store
                .shutdown()
                .await
                .map_err(|e| LupusError::Ipfs(format!("FsStore::shutdown: {e:?}")))?;
            tracing::info!("Blob store shut down cleanly");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IpfsConfig;
    use std::path::PathBuf;

    /// Tests share the process-wide BLOB_STORE so they must serialize.
    static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn temp_cache_dir(tag: &str) -> PathBuf {
        let pid = std::process::id();
        let base = std::env::temp_dir().join(format!("lupus-blob-test-{pid}-{tag}"));
        let _ = std::fs::remove_dir_all(&base); // ignore error if not exists
        base
    }

    async fn install_store(tag: &str) -> IpfsClient {
        let cache_dir = temp_cache_dir(tag);
        let cfg = IpfsConfig {
            enabled: true,
            gateway: String::new(),
            cache_dir,
            max_cache_gb: 5,
        };
        let mut client = IpfsClient::new(&cfg);
        client.connect().await.expect("blob store should open");
        client
    }

    #[tokio::test]
    async fn round_trip_add_then_get() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let _client = install_store("round_trip").await;

        let payload = b"<!doctype html><h1>hello den</h1>";
        let cid = add_blob(payload).await.expect("add ok");
        assert_eq!(cid.len(), 64, "expected 64-char hex hash, got {cid:?}");

        let fetched = get_blob(&cid).await.expect("get ok");
        let bytes = fetched.expect("blob should be present after add");
        assert_eq!(bytes.as_ref(), payload);
    }

    #[tokio::test]
    async fn add_is_content_addressed() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let _client = install_store("content_addr").await;

        // Identical bytes → identical CIDs (BLAKE3 deduplication).
        let cid_a = add_blob(b"same content").await.expect("add ok");
        let cid_b = add_blob(b"same content").await.expect("add ok");
        assert_eq!(cid_a, cid_b);

        // Different bytes → different CIDs.
        let cid_c = add_blob(b"different content").await.expect("add ok");
        assert_ne!(cid_a, cid_c);
    }

    #[tokio::test]
    async fn get_unknown_cid_returns_none() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let _client = install_store("unknown_cid").await;

        // 64 zero bytes — valid hex hash format, but not stored.
        let unknown_cid = "0".repeat(64);
        let result = get_blob(&unknown_cid).await.expect("get ok");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_with_invalid_cid_length_errors() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let _client = install_store("bad_cid_len").await;

        // Length neither 64 (hex) nor 52 (base32) — caught by our pre-check
        // before iroh-blobs' Hash::from_str (which would otherwise panic
        // inside data-encoding 2.10.0 on arbitrary input).
        let err = get_blob("too-short").await.unwrap_err();
        assert_eq!(err.code(), crate::protocol_codes::ERR_IPFS);
        let msg = format!("{err}");
        assert!(msg.contains("invalid CID"), "got: {msg}");
    }

    #[tokio::test]
    async fn get_with_invalid_hex_chars_errors() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        let _client = install_store("bad_cid_chars").await;

        // Length is 64 (hex slot) but the chars aren't valid hex.
        // iroh-blobs' Hash::from_str returns Err for this cleanly
        // (the panic was specifically on length-mismatched inputs).
        let invalid = "z".repeat(64);
        let err = get_blob(&invalid).await.unwrap_err();
        assert_eq!(err.code(), crate::protocol_codes::ERR_IPFS);
    }

    #[tokio::test]
    async fn add_without_loaded_store_errors() {
        let _guard = TEST_MUTEX.lock().await;
        reset_for_test().await;

        // Don't install — slot is empty.
        let err = add_blob(b"x").await.unwrap_err();
        assert_eq!(err.code(), crate::protocol_codes::ERR_IPFS);
        let msg = format!("{err}");
        assert!(msg.contains("not loaded"), "got: {msg}");
    }
}
