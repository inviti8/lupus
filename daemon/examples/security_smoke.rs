//! Security model smoke test — load `dist/lupus-security/` via candle and
//! classify a handful of URLs end-to-end.
//!
//! This is the equivalent of `tools/test_security_model.py` but going
//! through the daemon's pure-Rust path. The goal is to confirm:
//!   - The Qwen2 safetensors trunk loads cleanly
//!   - The `score.weight` classification head is read from the root prefix
//!   - The tokenizer reproduces the training-time `URL: <url>` format
//!   - Probabilities for known-safe vs. known-suspicious URLs come out
//!     in the right buckets (sanity check, not a full eval)
//!
//! Run with:
//!   cargo run --example security_smoke
//!
//! Note this loads ~2 GB of weights via mmap on first run; subsequent
//! cold starts are page-cache fast on the same boot.

use std::path::PathBuf;
use std::time::Instant;

use lupus::config::ModelsConfig;
use lupus::security::{run_full_scan, SecurityScanner};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,lupus=debug")),
        )
        .init();

    // Point at the in-repo dev model. The default config path resolves to
    // %LOCALAPPDATA%\lupus\models\lupus-security which won't exist on a
    // dev machine — we override directly here so the smoke test is
    // self-contained.
    let model_dir = PathBuf::from("dist/lupus-security");
    if !model_dir.exists() {
        eprintln!(
            "ERROR: model dir not found: {} (run from D:\\repos\\lupus root)",
            model_dir.display()
        );
        std::process::exit(2);
    }
    println!("=== loading security model from {} ===", model_dir.display());

    let models_cfg = ModelsConfig {
        search_base: PathBuf::new(),
        search_adapter: PathBuf::new(),
        content_adapter: PathBuf::new(),
        security: model_dir,
    };

    let mut scanner = SecurityScanner::new(&models_cfg);
    let load_start = Instant::now();
    if let Err(e) = scanner.load().await {
        eprintln!("ERROR: failed to load security model: {}", e);
        std::process::exit(3);
    }
    println!("=== loaded in {:.2}s ===", load_start.elapsed().as_secs_f32());

    // A small sanity panel — these aren't ground-truth labels but they
    // give us a quick read on whether the classifier is in a sane regime.
    let cases: &[(&str, &str)] = &[
        ("safe-ish",     "https://www.wikipedia.org/wiki/Cryptography"),
        ("safe-ish",     "https://github.com/rust-lang/rust"),
        ("safe-ish",     "https://docs.python.org/3/library/asyncio.html"),
        ("phishing-ish", "http://faceb00k-login.tk/login.php?session=abc123"),
        ("phishing-ish", "http://paypa1-secure-update.xyz/account/verify"),
        ("phishing-ish", "https://login-microsoft-account.click/Office365/auth.html"),
        ("malware-ish",  "http://192.168.4.66/cgi-bin/exec.cgi?cmd=/bin/sh"),
        ("malware-ish",  "http://download.evil.ru/payload.exe"),
    ];

    println!();
    println!(
        "{:<14}  {:<70}  {:>8}  {:>10}  {:>8}",
        "expect", "url", "p_safe", "p_phishing", "p_malware"
    );
    println!("{}", "-".repeat(118));

    for (label, url) in cases {
        let inf_start = Instant::now();
        let (score, threats) = run_full_scan(url, "").await;
        let elapsed_ms = inf_start.elapsed().as_millis();

        // Pull the model probabilities out of the threats list — we don't
        // expose them directly through the public API but each model
        // threat description carries `(p=0.xx)`.
        let phishing_p = extract_prob(&threats, "phishing_model");
        let malware_p = extract_prob(&threats, "malware_model");
        let safe_p = 1.0 - phishing_p - malware_p;

        let display_url = if url.len() > 68 { &url[..68] } else { url };
        println!(
            "{:<14}  {:<70}  {:>8.3}  {:>10.3}  {:>8.3}    score={:>3}  ({}ms)",
            label, display_url, safe_p.max(0.0), phishing_p, malware_p, score, elapsed_ms
        );
    }

    println!();
    println!("=== smoke test done ===");
}

/// Recover the probability that the model logged inside a threat
/// description like `"... flagged URL as phishing (p=0.92)"`. Returns
/// 0.0 if no matching threat exists (i.e. model said the class was
/// below the 0.5 confidence threshold).
fn extract_prob(threats: &[lupus::protocol::ThreatIndicator], kind: &str) -> f32 {
    for t in threats {
        if t.kind == kind {
            if let Some(start) = t.description.rfind("(p=") {
                if let Some(end) = t.description[start..].find(')') {
                    let s = &t.description[start + 3..start + end];
                    if let Ok(p) = s.parse::<f32>() {
                        return p;
                    }
                }
            }
        }
    }
    0.0
}
