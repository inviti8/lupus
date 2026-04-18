//! Joinner golden eval — single-tool-scoped (URL-bar search path).
//!
//! Runs the joinner against a curated set of `(query, scratchpad,
//! expectations)` cases and writes a human-readable markdown review at
//! `tests/fixtures/joinner_outputs.md` for manual grading. Also asserts
//! per-case substring rules so regressions flip the suite red.
//!
//! ## Running
//!
//! Gated behind `#[ignore]` because it loads the real 636 MB GGUF base
//! + 9 MB LoRA and takes 10-30 seconds per case on CPU (most of that
//! is the first-call cold-context warmup; subsequent cases are faster
//! once the model is mmap'd).
//!
//! ```bash
//! cd daemon
//! cargo test --test joinner_golden -- --ignored --nocapture
//! ```
//!
//! The markdown review is overwritten each run. Read it to grade output
//! quality — the assertions only catch gross regressions; the review is
//! what tells you if the model is doing something useful.
//!
//! ## Scope
//!
//! Per LUPUS_TOOLS.md, this eval covers ONLY the URL-bar search path:
//!   fetch_page → extract_content → joinner
//!
//! Other tool paths (scan_security, crawl_index, search_subnet,
//! search_local_index) get their own golden files as their model
//! behavior enters eval scope. Keep cases incremental — one tool path
//! at a time.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;

use lupus::agent::inference::InferenceEngine;
use lupus::agent::joinner::run_joinner_with_scratchpad;

// Model paths relative to the repo root (same defaults as
// `daemon/examples/agent_smoke.rs` — pulls straight from `dist/`).
fn base_model_path() -> PathBuf {
    repo_root().join("dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf")
}

fn lora_path() -> PathBuf {
    repo_root().join("dist/lupus-tinyagent-search/adapter.gguf")
}

fn repo_root() -> PathBuf {
    // `cargo test` sets CARGO_MANIFEST_DIR to the daemon crate root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().to_path_buf()
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

#[derive(Debug, Deserialize)]
struct Fixture {
    #[serde(default)]
    #[allow(dead_code)]
    _meta: serde_json::Value,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize, Clone)]
struct Case {
    id: String,
    query: String,
    scratchpad: String,
    expected_class: String,
    #[serde(default)]
    must_contain_any: Vec<String>,
    #[serde(default)]
    must_not_contain: Vec<String>,
    min_chars: usize,
    max_chars: usize,
}

struct CaseResult {
    case: Case,
    raw: String,
    thought: String,
    answer: String,
    #[allow(dead_code)]
    is_replan: bool,
    elapsed_ms: u128,
    passed: bool,
    failures: Vec<String>,
}

fn evaluate(case: &Case, raw: &str, thought: &str, answer: &str, is_replan: bool) -> (bool, Vec<String>) {
    let mut failures = Vec::new();
    let lower = answer.to_lowercase();

    if is_replan {
        failures.push("unexpected Replan output".to_string());
    }

    if answer.chars().count() < case.min_chars {
        failures.push(format!(
            "answer too short: {} chars (min {})",
            answer.chars().count(),
            case.min_chars
        ));
    }
    if answer.chars().count() > case.max_chars {
        failures.push(format!(
            "answer too long: {} chars (max {})",
            answer.chars().count(),
            case.max_chars
        ));
    }

    if !case.must_contain_any.is_empty() {
        let hit = case
            .must_contain_any
            .iter()
            .any(|needle| lower.contains(&needle.to_lowercase()));
        if !hit {
            failures.push(format!(
                "must_contain_any: none of {:?} found in answer",
                case.must_contain_any
            ));
        }
    }

    for needle in &case.must_not_contain {
        if lower.contains(&needle.to_lowercase()) {
            failures.push(format!("must_not_contain: found {:?} in answer", needle));
        }
    }

    // raw/thought captured for debugging only — no assertions on them
    let _ = raw;
    let _ = thought;

    (failures.is_empty(), failures)
}

fn write_markdown_review(results: &[CaseResult], path: &Path) -> std::io::Result<()> {
    use std::fmt::Write;
    let mut md = String::new();

    let total = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let mean_ms = if total > 0 {
        results.iter().map(|r| r.elapsed_ms).sum::<u128>() / total as u128
    } else {
        0
    };

    writeln!(md, "# Joinner golden eval — review\n").unwrap();
    writeln!(
        md,
        "**Pass:** {}/{}   **Mean latency:** {} ms   **Scope:** URL-bar search path (fetch_page → extract_content → joinner)\n",
        passed, total, mean_ms
    ).unwrap();
    writeln!(
        md,
        "_Assertions catch gross regressions. Read the `Answer` blocks below to grade quality. The `expected_class` tag describes what the case is testing for — match it against the model's posture._\n"
    ).unwrap();
    writeln!(md, "---\n").unwrap();

    for r in results {
        let status = if r.passed { "✅ PASS" } else { "❌ FAIL" };
        writeln!(
            md,
            "## {}  `{}`  ({} ms)  — expected `{}`\n",
            status, r.case.id, r.elapsed_ms, r.case.expected_class
        ).unwrap();

        writeln!(md, "**Query:** `{}`\n", r.case.query).unwrap();

        writeln!(md, "**Scratchpad:**").unwrap();
        writeln!(md, "```").unwrap();
        writeln!(md, "{}", r.case.scratchpad.trim_end()).unwrap();
        writeln!(md, "```\n").unwrap();

        writeln!(md, "**Raw joinner output:**").unwrap();
        writeln!(md, "```").unwrap();
        writeln!(md, "{}", r.raw).unwrap();
        writeln!(md, "```\n").unwrap();

        writeln!(md, "**Parsed answer ({} chars):**\n", r.answer.chars().count()).unwrap();
        writeln!(md, "> {}\n", r.answer.replace('\n', "\n> ")).unwrap();

        if !r.thought.is_empty() && r.thought != r.answer {
            writeln!(md, "**Thought:** {}\n", r.thought).unwrap();
        }

        if !r.failures.is_empty() {
            writeln!(md, "**Failures:**").unwrap();
            for f in &r.failures {
                writeln!(md, "- {}", f).unwrap();
            }
            writeln!(md).unwrap();
        }

        writeln!(md, "---\n").unwrap();
    }

    fs::write(path, md)?;
    Ok(())
}

#[test]
#[ignore = "loads the real 636 MB model; run with --ignored"]
fn joinner_search_path_golden() {
    let base = base_model_path();
    let lora = lora_path();
    assert!(
        base.exists(),
        "Base model not found at {}. Pull it via `python training/pull_model.py --model tinyagent` or adjust path.",
        base.display()
    );
    assert!(
        lora.exists(),
        "Search LoRA not found at {}. Pull via S3 or adjust path.",
        lora.display()
    );

    let fixture_path = fixtures_dir().join("joinner_cases.json");
    let fixture_raw = fs::read_to_string(&fixture_path).expect("fixture file");
    let fixture: Fixture = serde_json::from_str(&fixture_raw).expect("fixture parse");
    assert!(!fixture.cases.is_empty(), "no cases in fixture");

    println!(
        "Loading inference engine (base {}, lora {})...",
        base.display(),
        lora.display()
    );
    let t_load = Instant::now();
    let mut engine = InferenceEngine::load(&base, &lora).expect("load model");
    println!("Engine loaded in {:.1}s", t_load.elapsed().as_secs_f32());

    let mut results: Vec<CaseResult> = Vec::with_capacity(fixture.cases.len());

    for case in &fixture.cases {
        println!("\n-- case: {} --", case.id);
        let t = Instant::now();
        let out = run_joinner_with_scratchpad(&mut engine, &case.query, &case.scratchpad)
            .expect("run_joinner_with_scratchpad");
        let elapsed_ms = t.elapsed().as_millis();

        let (passed, failures) = evaluate(
            case,
            &out.raw_output,
            &out.thought,
            &out.answer,
            out.is_replan,
        );

        println!("   elapsed: {} ms", elapsed_ms);
        println!("   answer:  {:?}", out.answer);
        if !passed {
            println!("   failures: {:?}", failures);
        }

        results.push(CaseResult {
            case: case.clone(),
            raw: out.raw_output,
            thought: out.thought,
            answer: out.answer,
            is_replan: out.is_replan,
            elapsed_ms,
            passed,
            failures,
        });
    }

    // Write the review markdown regardless of pass/fail so you can read it.
    let review_path = fixtures_dir().join("joinner_outputs.md");
    write_markdown_review(&results, &review_path).expect("write review md");
    println!("\nReview written to {}", review_path.display());

    // Report summary, then fail if any case failed so CI / --ignored
    // runs flag regressions clearly.
    let total = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    println!("\n=== Result: {}/{} cases passed ===", passed, total);

    let failed: Vec<&CaseResult> = results.iter().filter(|r| !r.passed).collect();
    if !failed.is_empty() {
        for r in &failed {
            println!(" - FAIL {}: {:?}", r.case.id, r.failures);
        }
        panic!(
            "{}/{} cases failed. See {} for the full transcript.",
            failed.len(),
            total,
            review_path.display()
        );
    }
}
