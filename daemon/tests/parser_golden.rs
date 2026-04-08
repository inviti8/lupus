//! Golden-fixture test for the LLMCompiler plan parser.
//!
//! Loads the 22 model outputs from
//! `eval/tinyagent_runs/20260408-111025-eval.jsonl` (the post-Step-C
//! reference run that produced 21/22 GREEN), parses each `raw_output`
//! through the Rust parser, and asserts the resulting structure matches
//! the `parsed` field that the Python parser emitted at the time the
//! LoRA was validated.
//!
//! The Python parser is the oracle. Any drift between the Rust and
//! Python parsers fails this test, indicating the daemon may not extract
//! plans correctly from the trained planner's output.
//!
//! ## What we check
//!
//! For each model output in the fixture:
//!
//! 1. Same number of parsed steps
//! 2. For each step (in order):
//!    - Same `idx`
//!    - Same `name`
//!    - Same number of args (arity match — this is the metric the eval
//!      uses for "arg shape validity")
//!    - Same `references` list (the `$N` indices the executor will
//!      need to resolve at runtime)
//!
//! We deliberately don't compare the *parsed values* of args char-for-
//! char against the Python repr (Python's `parsed_args` field stores
//! `["'wolves'", '10']` which is `repr()`'d strings — awkward to match
//! exactly across the language boundary). The unit tests in
//! `daemon/src/agent/plan.rs::tests` handle exact value matching for a
//! curated subset; this fixture covers structural correctness across
//! every real model output we have.

use std::fs;
use std::path::PathBuf;

use lupus::agent::plan::parse_plan;
use serde_json::Value;

const FIXTURE_PATH: &str = "../eval/tinyagent_runs/20260408-111025-eval.jsonl";

#[test]
fn parser_matches_python_oracle_on_all_22_eval_cases() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_path = manifest_dir.join(FIXTURE_PATH);

    let raw = fs::read_to_string(&fixture_path).unwrap_or_else(|e| {
        panic!(
            "failed to read fixture at {}: {e}\n\
             this fixture is checked into git via .gitignore exception",
            fixture_path.display()
        )
    });

    let mut total_cases = 0;
    let mut total_steps = 0;
    let mut mismatches: Vec<String> = Vec::new();

    for (line_no, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        total_cases += 1;
        let record: Value = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!("line {}: invalid JSON: {e}", line_no + 1)
        });

        let case_id = record["case"]["id"].as_u64().unwrap_or(0);
        let query = record["case"]["query"].as_str().unwrap_or("");
        let raw_output = record["raw_output"]
            .as_str()
            .expect("raw_output should be a string");
        let python_parsed = record["parsed"]
            .as_array()
            .expect("parsed should be an array");

        // Run the Rust parser.
        let rust_steps = match parse_plan(raw_output) {
            Ok(s) => s,
            Err(e) => {
                mismatches.push(format!(
                    "case {case_id} ({query}): rust parser failed: {e}"
                ));
                continue;
            }
        };

        // Step-count match.
        if rust_steps.len() != python_parsed.len() {
            mismatches.push(format!(
                "case {case_id} ({query}): step count mismatch — rust={} python={}\n  raw_output: {raw_output:?}",
                rust_steps.len(),
                python_parsed.len()
            ));
            continue;
        }
        total_steps += rust_steps.len();

        // Per-step structural comparison.
        for (i, (rust_step, py_step)) in rust_steps.iter().zip(python_parsed.iter()).enumerate() {
            let py_idx = py_step["idx"].as_u64().unwrap_or(0) as u32;
            let py_name = py_step["name"].as_str().unwrap_or("");
            let py_args = py_step["parsed_args"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);
            let py_refs: Vec<u32> = py_step["references"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u32))
                        .collect()
                })
                .unwrap_or_default();

            if rust_step.idx != py_idx {
                mismatches.push(format!(
                    "case {case_id} step {i}: idx mismatch — rust={} python={}",
                    rust_step.idx, py_idx
                ));
            }
            if rust_step.name != py_name {
                mismatches.push(format!(
                    "case {case_id} step {i}: name mismatch — rust={:?} python={:?}",
                    rust_step.name, py_name
                ));
            }
            if rust_step.args.len() != py_args {
                mismatches.push(format!(
                    "case {case_id} step {i} ({}): arg count mismatch — rust={} python={}\n  raw_args: {:?}",
                    rust_step.name,
                    rust_step.args.len(),
                    py_args,
                    rust_step.raw_args
                ));
            }
            if rust_step.references != py_refs {
                mismatches.push(format!(
                    "case {case_id} step {i} ({}): references mismatch — rust={:?} python={:?}",
                    rust_step.name, rust_step.references, py_refs
                ));
            }
        }
    }

    assert_eq!(
        total_cases, 22,
        "expected 22 cases in the fixture, found {total_cases}"
    );

    if !mismatches.is_empty() {
        let detail = mismatches.join("\n  ");
        panic!(
            "Rust parser drifts from Python oracle on {} step(s) across {} case(s):\n  {}",
            mismatches.len(),
            total_cases,
            detail
        );
    }

    eprintln!(
        "Parser golden test passed: {total_steps} steps across {total_cases} cases match the Python oracle"
    );
}
