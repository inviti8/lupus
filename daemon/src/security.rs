//! Security scanner — Qwen2 v0.3 URL classifier + lookalike-domain heuristics.
//!
//! The model is `Qwen2ForSequenceClassification` (3 classes: safe / phishing
//! / malware) trained at `training/train_security.py` and shipped in
//! `dist/lupus-security/` as a HuggingFace safetensors directory. We load it
//! with **candle-transformers** rather than llama-cpp-2 because GGUF can't
//! represent classification heads — llama-cpp-2 only supports causal LMs.
//!
//! ## Architecture
//!
//! `SecurityClassifier` holds the Qwen2 trunk plus a custom `Linear(896, 3)`
//! built from the `score.weight` tensor in the safetensors. It exposes
//! `classify(&mut self, url: &str) -> [f32; 3]` which mirrors the Python
//! eval at `tools/test_security_model.py` exactly:
//!   - Input format: `format!("URL: {}", url)` (matching `train_security.py:140`)
//!   - max_length = 128 tokens
//!   - Take the hidden state of the last non-pad token and project through `score`
//!   - Return softmax over [safe, phishing, malware]
//!
//! ## Sharing
//!
//! `SecurityScanner::scan()` (the IPC handler path) and the `scan_security`
//! tool (the agent loop path) both read from a single global classifier
//! `CLASSIFIER`. Loading happens once at daemon startup via
//! `SecurityScanner::load()`. The lazy global pattern keeps tools as
//! stateless free functions while still sharing one ~2 GB model in memory.
//!
//! ## KV cache reset
//!
//! candle's `qwen2::Model` holds an internal KV cache per attention layer.
//! Each `classify()` call resets the cache via `Model::clear_kv_cache()`
//! before forwarding so consecutive calls don't accumulate state.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::{ops::softmax_last_dim, Activation, Linear, Module, VarBuilder};
use candle_transformers::models::qwen2::{Config as Qwen2Config, Model as Qwen2Model};
use tokenizers::Tokenizer;
use tokio::sync::Mutex;

use crate::config::ModelsConfig;
use crate::error::LupusError;
use crate::protocol::{ComponentState, ScanParams, ScanResponse, ThreatIndicator};

// ---------------------------------------------------------------------------
// Global classifier — one shared instance.
// ---------------------------------------------------------------------------

/// Single-process classifier slot. Initialized lazily on first access; the
/// inner `Option` is populated by `SecurityScanner::load()` at startup.
/// Both the IPC `scan_page` handler and the `scan_security` agent tool
/// read from this slot via `with_classifier`.
static CLASSIFIER: OnceLock<Mutex<Option<SecurityClassifier>>> = OnceLock::new();

fn classifier_slot() -> &'static Mutex<Option<SecurityClassifier>> {
    CLASSIFIER.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------
// SecurityScanner — facade owned by `Daemon`.
// ---------------------------------------------------------------------------

pub struct SecurityScanner {
    model_path: PathBuf,
}

impl SecurityScanner {
    pub fn new(models: &ModelsConfig) -> Self {
        Self {
            model_path: models.security.clone(),
        }
    }

    /// Load the security model into the global slot. Called once during
    /// daemon startup. If the model directory doesn't exist or fails to
    /// load, the daemon stays up in heuristic-only mode (a warning is
    /// logged and the slot stays empty).
    pub async fn load(&mut self) -> Result<(), LupusError> {
        tracing::info!("Loading security model from {}", self.model_path.display());

        if !self.model_path.exists() {
            return Err(LupusError::ModelLoadFailed(format!(
                "security model directory not found: {}",
                self.model_path.display()
            )));
        }

        let classifier = SecurityClassifier::new(&self.model_path)
            .map_err(|e| LupusError::ModelLoadFailed(format!("security: {e}")))?;

        *classifier_slot().lock().await = Some(classifier);
        tracing::info!("Security model loaded ({} labels)", 3);
        Ok(())
    }

    /// Scan a page (URL + HTML) and produce a trust score with threats.
    /// Used by the IPC `scan_page` method on `Daemon`.
    pub async fn scan(&self, params: ScanParams) -> Result<ScanResponse, LupusError> {
        let (score, threats) = run_full_scan(&params.url, &params.html).await;
        Ok(ScanResponse { score, threats })
    }

    /// Quick heuristic pre‑check. Static — runs without the model. Catches
    /// obvious threats (lookalike domains, password forms on non‑HTTPS,
    /// obfuscated JS) at zero inference cost.
    pub fn heuristic_scan(url: &str, html: &str) -> Vec<ThreatIndicator> {
        let mut threats = Vec::new();

        // Lookalike domain detection
        let suspicious_patterns = [
            "faceb00k", "g00gle", "amaz0n", "paypa1", "micros0ft",
        ];
        let url_lower = url.to_lowercase();
        for pattern in &suspicious_patterns {
            if url_lower.contains(pattern) {
                threats.push(ThreatIndicator {
                    kind: "lookalike_domain".into(),
                    description: format!("URL contains suspicious pattern: {}", pattern),
                    severity: "high".into(),
                });
            }
        }

        // Credential form on non‑HTTPS
        if !url.starts_with("https://") && html.contains("<input") && html.contains("password") {
            threats.push(ThreatIndicator {
                kind: "insecure_credentials".into(),
                description: "Password field on non-HTTPS page".into(),
                severity: "critical".into(),
            });
        }

        // Obfuscated JavaScript
        let obfuscation_signals = ["eval(atob(", "\\x65\\x76\\x61\\x6c", "String.fromCharCode"];
        for signal in &obfuscation_signals {
            if html.contains(signal) {
                threats.push(ThreatIndicator {
                    kind: "obfuscated_js".into(),
                    description: format!("Obfuscated JavaScript detected: {}", signal),
                    severity: "medium".into(),
                });
            }
        }

        threats
    }

    /// Status used by the get_status IPC handler. Synchronous — uses
    /// `try_lock` so it never blocks; on contention we report Loading.
    pub fn component_state(&self) -> ComponentState {
        match classifier_slot().try_lock() {
            Ok(guard) => {
                if guard.is_some() {
                    ComponentState::Ready
                } else {
                    ComponentState::Loading
                }
            }
            Err(_) => ComponentState::Loading,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared scan function — used by both the IPC path and the agent tool.
// ---------------------------------------------------------------------------

/// Run heuristics + the model classifier on a page and return
/// `(trust_score, threats)`. Both `SecurityScanner::scan` (IPC handler)
/// and `crate::tools::scan_security::execute` (agent tool) call this so
/// the two surfaces stay in lockstep.
pub async fn run_full_scan(url: &str, html: &str) -> (u8, Vec<ThreatIndicator>) {
    let mut threats = SecurityScanner::heuristic_scan(url, html);

    // Run model classifier (URL only — the v0.3 model is URL-only, see
    // training/train_security.py:140 — HTML is for heuristics only).
    let mut slot = classifier_slot().lock().await;
    if let Some(clf) = slot.as_mut() {
        match clf.classify(url) {
            Ok(probs) => {
                tracing::debug!(
                    "security probs url={} safe={:.3} phishing={:.3} malware={:.3}",
                    url, probs[0], probs[1], probs[2]
                );
                if let Some(t) = model_threat("phishing", probs[1]) {
                    threats.push(t);
                }
                if let Some(t) = model_threat("malware", probs[2]) {
                    threats.push(t);
                }
            }
            Err(e) => {
                tracing::warn!("security classifier error for {}: {}", url, e);
            }
        }
    }
    drop(slot);

    let score = score_from_threats(&threats);
    (score, threats)
}

/// Convert a class probability into a `ThreatIndicator` if it crosses the
/// confidence threshold. The thresholds are deliberately strict — the v0.3
/// eval reported f1_macro=0.9964 so high-confidence flags are reliable.
fn model_threat(kind: &str, prob: f32) -> Option<ThreatIndicator> {
    if prob < 0.5 {
        return None;
    }
    let severity = if prob >= 0.85 { "critical" } else { "high" };
    Some(ThreatIndicator {
        kind: format!("{}_model", kind),
        description: format!(
            "Security classifier flagged URL as {} (p={:.2})",
            kind, prob
        ),
        severity: severity.into(),
    })
}

/// Compute a 0–100 trust score by deducting per-severity penalties from
/// 100. Matches the formula previously inlined in `tools/scan_security.rs`
/// before this consolidation.
fn score_from_threats(threats: &[ThreatIndicator]) -> u8 {
    let deductions: u32 = threats
        .iter()
        .map(|t| match t.severity.as_str() {
            "critical" => 40,
            "high" => 25,
            "medium" => 10,
            "low" => 5,
            _ => 5,
        })
        .sum();
    100u32.saturating_sub(deductions).min(100) as u8
}

// ---------------------------------------------------------------------------
// SecurityClassifier — pure-Rust Qwen2 sequence classifier.
// ---------------------------------------------------------------------------

/// Qwen2-based 3-class URL classifier loaded from a HuggingFace safetensors
/// directory. Holds the trunk model, the classification head, and the
/// tokenizer. Single-instance, single-thread access via the `CLASSIFIER`
/// global mutex.
struct SecurityClassifier {
    model: Qwen2Model,
    score: Linear,
    tokenizer: Tokenizer,
    device: Device,
    pad_token_id: u32,
    max_length: usize,
}

impl SecurityClassifier {
    /// Load the model, tokenizer, config, and classification head from a
    /// HuggingFace safetensors directory. Expected files:
    /// `config.json`, `tokenizer.json`, `model.safetensors`.
    fn new(model_dir: &Path) -> Result<Self, String> {
        // CPU is the only universal backend on Windows; CUDA can be wired
        // later via a feature flag. F32 matches the trained checkpoint
        // bit-for-bit so eval parity is preserved.
        let device = Device::Cpu;
        let dtype = DType::F32;

        // --- tokenizer ---
        let tokenizer = Tokenizer::from_file(model_dir.join("tokenizer.json"))
            .map_err(|e| format!("load tokenizer: {e}"))?;

        // --- config (hand-built — HF's config.json shape is incompatible
        //     with candle's `Config` struct: rope_theta is nested under
        //     `rope_parameters`, and `sliding_window` is `null`) ---
        let cfg_path = model_dir.join("config.json");
        let cfg_bytes = std::fs::read(&cfg_path)
            .map_err(|e| format!("read config.json: {e}"))?;
        let cfg_json: serde_json::Value = serde_json::from_slice(&cfg_bytes)
            .map_err(|e| format!("parse config.json: {e}"))?;

        let hidden_size = json_usize(&cfg_json, "hidden_size", 896);
        let max_pos = json_usize(&cfg_json, "max_position_embeddings", 32768);

        let cfg = Qwen2Config {
            vocab_size: json_usize(&cfg_json, "vocab_size", 151936),
            hidden_size,
            intermediate_size: json_usize(&cfg_json, "intermediate_size", 4864),
            num_hidden_layers: json_usize(&cfg_json, "num_hidden_layers", 24),
            num_attention_heads: json_usize(&cfg_json, "num_attention_heads", 14),
            num_key_value_heads: json_usize(&cfg_json, "num_key_value_heads", 2),
            max_position_embeddings: max_pos,
            // HF has `"sliding_window": null` + `"use_sliding_window": false`,
            // but candle wants a concrete usize. Setting to max_pos makes the
            // sliding-window mask a no-op (it never masks anything).
            sliding_window: max_pos,
            max_window_layers: json_usize(&cfg_json, "max_window_layers", 24),
            tie_word_embeddings: cfg_json["tie_word_embeddings"].as_bool().unwrap_or(true),
            // HF nests rope_theta under rope_parameters
            rope_theta: cfg_json["rope_parameters"]["rope_theta"]
                .as_f64()
                .or_else(|| cfg_json["rope_theta"].as_f64())
                .unwrap_or(1_000_000.0),
            rms_norm_eps: cfg_json["rms_norm_eps"].as_f64().unwrap_or(1e-6),
            use_sliding_window: cfg_json["use_sliding_window"].as_bool().unwrap_or(false),
            hidden_act: Activation::Silu,
        };

        let pad_token_id = cfg_json["pad_token_id"].as_u64().unwrap_or(151643) as u32;

        // --- weights ---
        // mmap is `unsafe` because it relies on the file not being mutated
        // under us. The model directory is read-only at runtime so this
        // is safe in our deployment.
        let weights_path = model_dir.join("model.safetensors");
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], dtype, &device)
                .map_err(|e| format!("mmap safetensors: {e}"))?
        };

        // Qwen2Model::new internally calls `vb.pp("model")`, so pass the
        // root vb (NOT vb.pp("model") — we'd double-prefix).
        let model = Qwen2Model::new(&cfg, vb.clone())
            .map_err(|e| format!("build qwen2 model: {e}"))?;

        // Classification head: `score.weight` lives at the safetensors root
        // (no `model.` prefix), shape [3, hidden_size], no bias. This is the
        // distinguishing feature of `Qwen2ForSequenceClassification` vs.
        // `Qwen2ForCausalLM` (which has `lm_head.weight` instead).
        let score_w = vb
            .get((3, hidden_size), "score.weight")
            .map_err(|e| format!("load score.weight: {e}"))?;
        let score = Linear::new(score_w, None);

        Ok(Self {
            model,
            score,
            tokenizer,
            device,
            pad_token_id,
            max_length: 128,
        })
    }

    /// Classify a URL into [safe, phishing, malware] probabilities.
    /// Resets the model's internal KV cache before forwarding so
    /// consecutive calls don't accumulate state.
    fn classify(&mut self, url: &str) -> Result<[f32; 3], String> {
        // Match `training/train_security.py:140` — `text = f"URL: {ex['url']}"`
        let text = format!("URL: {}", url);

        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("tokenize: {e}"))?;

        let mut ids: Vec<u32> = enc.get_ids().to_vec();
        if ids.len() > self.max_length {
            ids.truncate(self.max_length);
        }
        if ids.is_empty() {
            return Err("empty tokenization".into());
        }
        // HF Qwen2ForSequenceClassification reduces over the LAST non-pad
        // token. We don't pad (batch size 1) so this is just `len - 1`,
        // but we still strip any trailing pad to be safe.
        while ids.len() > 1 && ids.last() == Some(&self.pad_token_id) {
            ids.pop();
        }
        let last_idx = ids.len() - 1;

        // input_ids: [1, seq_len], dtype U32
        let input_ids = Tensor::new(ids.as_slice(), &self.device)
            .map_err(|e| format!("build input_ids: {e}"))?
            .unsqueeze(0)
            .map_err(|e| format!("unsqueeze input_ids: {e}"))?;

        // Reset KV cache so consecutive classify() calls are independent.
        self.model.clear_kv_cache();

        // Forward through the trunk. seqlen_offset=0 because we're not
        // continuing a previous generation; attn_mask=None lets the model
        // build its own causal mask internally.
        let hidden = self
            .model
            .forward(&input_ids, 0, None)
            .map_err(|e| format!("model forward: {e}"))?;
        // hidden: [1, seq_len, hidden_size]

        // Pool: take the last non-pad token's hidden state.
        let pooled = hidden
            .i((0, last_idx, ..))
            .map_err(|e| format!("pool last token: {e}"))?
            .unsqueeze(0)
            .map_err(|e| format!("unsqueeze pooled: {e}"))?;

        // Project through the classification head: [1, hidden] @ [3, hidden]^T → [1, 3]
        let logits = self
            .score
            .forward(&pooled)
            .map_err(|e| format!("score head: {e}"))?;

        let probs = softmax_last_dim(&logits)
            .map_err(|e| format!("softmax: {e}"))?;
        let v: Vec<f32> = probs
            .squeeze(0)
            .and_then(|t| t.to_dtype(DType::F32))
            .and_then(|t| t.to_vec1())
            .map_err(|e| format!("extract probs: {e}"))?;

        if v.len() != 3 {
            return Err(format!("expected 3 probs, got {}", v.len()));
        }
        Ok([v[0], v[1], v[2]])
    }
}

/// Helper to read a usize from `cfg_json` with a fallback default.
fn json_usize(v: &serde_json::Value, key: &str, default: usize) -> usize {
    v[key].as_u64().map(|n| n as usize).unwrap_or(default)
}
