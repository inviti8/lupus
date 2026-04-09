//! Full agent smoke test — `Agent::load` + `Agent::hunt` end-to-end.
//!
//! This is the daemon-side equivalent of `python tools/eval_tinyagent.py`
//! for a single query. It exercises the entire LLMCompiler agent loop:
//!
//! 1. Load the base GGUF + trained search LoRA into an `InferenceEngine`
//! 2. Run the planner with the LoRA attached → raw plan text
//! 3. Parse the plan via the LLMCompiler parser
//! 4. Execute the plan against the (currently stubbed) tool dispatcher
//! 5. Run the joinner WITHOUT the LoRA → `Action: Finish(<answer>)`
//! 6. Build and print the final `SearchResponse`
//!
//! Usage:
//!     cargo run --example agent_smoke
//!     cargo run --example agent_smoke -- "your query here"
//!
//! Run from the dev shell so cmake/cl/libclang are on PATH.
//!
//! The tools currently return empty stubs (search returns `{"results": []}`,
//! fetch returns `{"body": "", "status": "not_implemented"}`, etc.) so
//! the joinner sees a plan with empty observations and produces a
//! "no results found"-flavored answer. That's expected for v1 — the
//! point of this smoke test is to validate the full plumbing, not
//! the tool implementations.

use std::path::PathBuf;
use std::time::Instant;

use lupus::agent::Agent;
use lupus::config::ModelsConfig;
use lupus::protocol::SearchParams;

const DEFAULT_QUERY: &str = "Find pages about wolves in my local index";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .ok_or("CARGO_MANIFEST_DIR has no parent")?;

    let models = ModelsConfig {
        search_base: repo_root.join("dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf"),
        search_adapter: repo_root.join("dist/lupus-tinyagent-search/adapter.gguf"),
        content_adapter: repo_root.join("dist/lupus-tinyagent-content/adapter.gguf"),
        security: repo_root.join("dist/lupus-security"),
    };

    println!("Model paths:");
    println!("  base GGUF:    {}", models.search_base.display());
    println!("  search LoRA:  {}", models.search_adapter.display());
    println!();

    let query = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_QUERY.to_string());
    println!("Query: {query}");
    println!();

    print!("Loading agent... ");
    let load_start = Instant::now();
    let mut agent = Agent::new(&models);
    agent.load().await?;
    let load_elapsed = load_start.elapsed();
    println!("done in {:.2}s", load_elapsed.as_secs_f64());
    println!();

    let params = SearchParams {
        query: query.clone(),
        scope: None,
    };

    print!("Running full agent pipeline (planner + execute + joinner)... ");
    let infer_start = Instant::now();
    let response = agent.hunt(params).await?;
    let infer_elapsed = infer_start.elapsed();
    println!("done in {:.2}s", infer_elapsed.as_secs_f64());
    println!();

    println!("=== SearchResponse ===");
    if let Some(answer) = &response.text_answer {
        println!("text_answer:");
        for line in answer.lines() {
            println!("  | {line}");
        }
    } else {
        println!("text_answer: (none)");
    }
    println!();

    if let Some(plan) = &response.plan {
        println!("plan ({} steps):", plan.len());
        for step in plan {
            let kind = if step.is_join { "join" } else { "tool" };
            println!(
                "  {}. [{}] {}({})",
                step.idx, kind, step.tool, step.raw_args
            );
            if let Some(obs) = &step.observation {
                let obs_str = obs.to_string();
                let truncated = if obs_str.len() > 200 {
                    format!("{}... [truncated]", &obs_str[..200])
                } else {
                    obs_str
                };
                println!("     observation: {truncated}");
            }
            if let Some(err) = &step.error {
                println!("     error: {err}");
            }
        }
    } else {
        println!("plan: (none)");
    }
    println!();

    println!("results: {} hit(s)", response.results.len());
    for (i, r) in response.results.iter().enumerate() {
        println!("  {}. {} ({})", i + 1, r.title, r.url);
    }
    println!("======================");

    Ok(())
}
