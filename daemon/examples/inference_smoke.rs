//! Inference smoke test — load the base GGUF + trained LoRA adapter,
//! call the planner once on a known query, print the raw model output.
//!
//! This is the daemon-side equivalent of `python tools/tinyagent_prompt_probe.py`
//! and is the side-by-side comparison gate for Phase 3 of the daemon
//! integration plan. The query and expected behavior are the same as the
//! Python probe; output should match within the limits of greedy sampling
//! (which is deterministic given identical inputs and identical
//! prompts — the SHA-256 snapshot test in `agent/prompt.rs` already
//! verified the prompts match).
//!
//! Usage:
//!     cargo run --example inference_smoke
//!     cargo run --example inference_smoke -- "your query here"
//!
//! Run from the dev shell (`daemon/scripts/devshell.ps1`) so cmake/cl/libclang
//! are on PATH for the first build.
//!
//! Reference Python output for the default query:
//!     .
//!     1. search_local_index("wolves", 10)
//!     2. join()

use std::path::PathBuf;
use std::time::Instant;

// Re-import the agent's prompt + inference modules from the daemon binary
// crate. The `lupus::` path is the binary crate's name from Cargo.toml.
use lupus::agent::inference::{InferenceEngine, MAX_PLANNER_TOKENS};
use lupus::agent::prompt::planner_system_prompt;

const DEFAULT_QUERY: &str = "Find pages about wolves in my local index";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Resolve model paths relative to the repo root (two levels up from
    // daemon/Cargo.toml). The example is launched via `cargo run --example`
    // from the daemon dir, so CARGO_MANIFEST_DIR is daemon/.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .ok_or("CARGO_MANIFEST_DIR has no parent")?;

    let model_path = repo_root.join("dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf");
    let lora_path = repo_root.join("dist/lupus-tinyagent-search/adapter.gguf");

    println!("Model paths:");
    println!("  base GGUF: {}", model_path.display());
    println!("  search LoRA: {}", lora_path.display());
    println!();

    let query = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_QUERY.to_string());
    println!("Query: {}", query);
    println!();

    // Load the model + LoRA. This is the slow step (~1-2 s for the GGUF
    // mmap, plus a few hundred ms for the LoRA).
    print!("Loading model + LoRA... ");
    let load_start = Instant::now();
    let mut engine = InferenceEngine::load(&model_path, &lora_path)?;
    let load_elapsed = load_start.elapsed();
    println!("done in {:.2}s", load_elapsed.as_secs_f64());
    println!();

    // Render the canonical system prompt (built once at first call,
    // cached in OnceLock for subsequent calls).
    let system_prompt = planner_system_prompt();
    println!("System prompt: {} bytes", system_prompt.len());
    println!();

    // Run the inference. Greedy sampling, stop on END_OF_PLAN / </s> / etc.
    let user_prompt = format!("Question: {query}");
    print!("Running planner inference... ");
    let infer_start = Instant::now();
    let raw_output = engine.infer_blocking(system_prompt, &user_prompt, MAX_PLANNER_TOKENS)?;
    let infer_elapsed = infer_start.elapsed();
    println!("done in {:.2}s", infer_elapsed.as_secs_f64());
    println!();

    println!("=== raw model output ===");
    for line in raw_output.lines() {
        println!("  | {line}");
    }
    println!("========================");
    println!();
    println!("Total chars: {}", raw_output.len());

    Ok(())
}
