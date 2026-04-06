//! Security scanner — Qwen2.5‑Coder for HTML/JS threat analysis.
//!
//! Loaded at startup, always resident in memory. Runs on every page load
//! with a latency target of < 500ms.

use crate::config::ModelsConfig;
use crate::error::LupusError;
use crate::protocol::{ComponentState, ScanParams, ScanResponse, ThreatIndicator};

pub struct SecurityScanner {
    model_path: std::path::PathBuf,
    state: ScannerState,
}

enum ScannerState {
    Unloaded,
    Ready,
    // TODO: Ready { model: LlamaModel }
}

impl SecurityScanner {
    pub fn new(models: &ModelsConfig) -> Self {
        Self {
            model_path: models.security.clone(),
            state: ScannerState::Unloaded,
        }
    }

    /// Load the security model. Called first during startup — the browser
    /// needs trust scores before anything else.
    pub async fn load(&mut self) -> Result<(), LupusError> {
        tracing::info!("Loading security model from {}", self.model_path.display());

        // TODO: Load GGUF via llama-cpp-2
        //   let model = LlamaModel::load_from_file(&self.model_path, params)?;

        self.state = ScannerState::Ready;
        tracing::info!("Security model loaded");
        Ok(())
    }

    /// Scan raw HTML + URL and produce a trust score with threat indicators.
    pub async fn scan(&self, params: ScanParams) -> Result<ScanResponse, LupusError> {
        self.require_ready()?;

        tracing::debug!("Scanning page: {}", params.url);

        // TODO: Build prompt from HTML + URL, run inference, parse output
        //   let prompt = format_security_prompt(&params.html, &params.url);
        //   let output = model.generate(&prompt, max_tokens)?;
        //   parse_security_output(&output)

        // Stub: return safe score with no threats
        Ok(ScanResponse {
            score: 100,
            threats: Vec::new(),
        })
    }

    /// Quick heuristic pre‑check before running the model. Catches obvious
    /// threats without inference cost.
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

    pub fn component_state(&self) -> ComponentState {
        match &self.state {
            ScannerState::Ready => ComponentState::Ready,
            ScannerState::Unloaded => ComponentState::Loading,
        }
    }

    fn require_ready(&self) -> Result<(), LupusError> {
        match &self.state {
            ScannerState::Ready => Ok(()),
            ScannerState::Unloaded => Err(LupusError::ModelNotLoaded("security".into())),
        }
    }
}
