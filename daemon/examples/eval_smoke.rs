//! Phase 7 parity check: run all 22 test cases through the daemon's
//! planner pipeline and assert the metrics match the Python eval's
//! 21/22 GREEN result (`eval/tinyagent_runs/20260408-111025-eval.jsonl`).
//!
//! Mirrors `tools/eval_tinyagent.py` but stays in Rust:
//! - Same 22 hardcoded test cases (id, query, expected_tools, multi_step, allow_abstain)
//! - Same 6 metrics (validity, tool selection, arg shape, hallucination, multi-step, abstention)
//! - Same TOOL_ARITY table for arg-shape validation
//! - Same scoring logic (set comparison for tools, count check for arity, etc.)
//!
//! We deliberately skip the joinner — the parity check is for the
//! **planner**'s output (which is what the LoRA was trained for and
//! what the Python eval scores). The daemon's full agent loop adds the
//! joinner on top, but the joinner correctness is a separate concern
//! and the trained LoRA's quality contract is on the planner.
//!
//! Usage:
//!     cargo run --example eval_smoke --release
//!
//! Run with --release for the ~2x inference speedup. Even so, this
//! takes 12-18 minutes wall clock on CPU because each case is a fresh
//! LlamaContext + planner inference. Acceptable for a one-time parity
//! check; not intended for tight iteration loops.
//!
//! Acceptance gate: tool selection >= 80% AND hallucination <= 5%.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use lupus::agent::inference::{InferenceEngine, MAX_PLANNER_TOKENS};
use lupus::agent::plan::{self, PlanArg, PlanStep};
use lupus::agent::prompt::planner_system_prompt;

// ---------------------------------------------------------------------------
// Test cases — verbatim from tools/eval_tinyagent.py::TEST_CASES.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TestCase {
    id: u32,
    query: &'static str,
    expected_tools: &'static [&'static str],
    multi_step: bool,
    allow_abstain: bool,
    requires_dependency: bool,
    /// Alternative tool sets that should also count as a correct
    /// tool selection (e.g. case 11 accepts either {scan_security}
    /// or {fetch_page, scan_security}).
    acceptable_alts: &'static [&'static [&'static str]],
}

const TEST_CASES: &[TestCase] = &[
    // Single-tool: search_local_index (5)
    TestCase { id: 1, query: "Find pages about wolves in my local index",
        expected_tools: &["search_local_index"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 2, query: "What did I save about Anishinaabe folklore?",
        expected_tools: &["search_local_index"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 3, query: "Search my local history for rust borrow checker explanations",
        expected_tools: &["search_local_index"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 4, query: "Any pages mentioning IPFS content routing?",
        expected_tools: &["search_local_index"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 5, query: "Show me saved articles about wool felting",
        expected_tools: &["search_local_index"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    // Single-tool: search_subnet (3)
    TestCase { id: 6, query: "Search the hvym cooperative for weaving datapods",
        expected_tools: &["search_subnet"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 7, query: "Find datapods about open-source 3D printing",
        expected_tools: &["search_subnet"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 8, query: "Is there a subnet entry for Lepus browser docs?",
        expected_tools: &["search_subnet"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    // Single-tool: fetch_page (2)
    TestCase { id: 9, query: "Fetch https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/",
        expected_tools: &["fetch_page"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 10, query: "Get the content of hvym://cooperative/weaving/intro",
        expected_tools: &["fetch_page"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    // Single-tool: scan_security (2) — both accept the chained alt
    TestCase { id: 11, query: "Is https://paypa1-secure.support/login.php safe?",
        expected_tools: &["scan_security"], multi_step: false,
        allow_abstain: false, requires_dependency: false,
        acceptable_alts: &[&["fetch_page", "scan_security"]] },
    TestCase { id: 12, query: "Check https://github.com/inviti8/lupus for threats",
        expected_tools: &["scan_security"], multi_step: false,
        allow_abstain: false, requires_dependency: false,
        acceptable_alts: &[&["fetch_page", "scan_security"]] },
    // Single-tool: crawl_index (1)
    TestCase { id: 13, query: "Add https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/ to my index",
        expected_tools: &["crawl_index"], multi_step: false,
        allow_abstain: false, requires_dependency: false, acceptable_alts: &[] },
    // Multi-step (5)
    TestCase { id: 14, query: "Summarize the BAIR TinyAgent blog post",
        expected_tools: &["fetch_page", "extract_content"], multi_step: true,
        allow_abstain: false, requires_dependency: true, acceptable_alts: &[] },
    TestCase { id: 15, query: "Fetch https://example.com/article.html and tell me if it's dangerous",
        expected_tools: &["fetch_page", "scan_security"], multi_step: true,
        allow_abstain: false, requires_dependency: true, acceptable_alts: &[] },
    TestCase { id: 16, query: "Find a cooperative datapod about weaving and save it to my index",
        expected_tools: &["search_subnet", "crawl_index"], multi_step: true,
        allow_abstain: false, requires_dependency: true, acceptable_alts: &[] },
    TestCase { id: 17, query: "Look up wolves in my local index, then fetch the first result in full",
        expected_tools: &["search_local_index", "fetch_page"], multi_step: true,
        allow_abstain: false, requires_dependency: true, acceptable_alts: &[] },
    TestCase { id: 18, query: "Fetch the Lupus GitHub README, extract its summary, and scan it for threats",
        expected_tools: &["fetch_page", "extract_content", "scan_security"], multi_step: true,
        allow_abstain: false, requires_dependency: true, acceptable_alts: &[] },
    // No-tool / abstention (2)
    TestCase { id: 19, query: "What is 2+2?",
        expected_tools: &[], multi_step: false, allow_abstain: true,
        requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 20, query: "Who are you?",
        expected_tools: &[], multi_step: false, allow_abstain: true,
        requires_dependency: false, acceptable_alts: &[] },
    // Adversarial / hallucination check (2)
    TestCase { id: 21, query: "Email my wife that I'll be late",
        expected_tools: &[], multi_step: false, allow_abstain: true,
        requires_dependency: false, acceptable_alts: &[] },
    TestCase { id: 22, query: "Open a terminal and run `ls`",
        expected_tools: &[], multi_step: false, allow_abstain: true,
        requires_dependency: false, acceptable_alts: &[] },
];

// Tool arity table — verbatim from tools/eval_tinyagent.py::TOOL_ARITY.
// Used by the arg-shape scoring metric: a call's len(parsed_args) must
// equal the expected arity for that tool.
fn tool_arity(name: &str) -> Option<usize> {
    match name {
        "search_subnet" => Some(2),
        "search_local_index" => Some(2),
        "fetch_page" => Some(1),
        "extract_content" => Some(2),
        "scan_security" => Some(2),
        "crawl_index" => Some(1),
        _ => None,
    }
}

const LUPUS_TOOL_NAMES: &[&str] = &[
    "search_subnet",
    "search_local_index",
    "fetch_page",
    "extract_content",
    "scan_security",
    "crawl_index",
];

// ---------------------------------------------------------------------------
// Per-case scoring (mirrors tools/eval_tinyagent.py::score_case)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CaseScore {
    id: u32,
    query: String,
    raw_output: String,
    observed_tools: Vec<String>,
    valid_syntax: bool,
    tool_selection_ok: bool,
    arg_shape_ok: bool,
    hallucinated: bool,
    multi_step_ok: bool,
    abstained_correctly: bool,
    dependency_ok: bool,
}

impl CaseScore {
    fn marker(&self) -> &'static str {
        let hard_pass = self.valid_syntax
            && self.tool_selection_ok
            && self.arg_shape_ok
            && !self.hallucinated
            && self.multi_step_ok
            && self.abstained_correctly
            && self.dependency_ok;
        if hard_pass {
            "ok"
        } else if self.valid_syntax && self.tool_selection_ok {
            "?"
        } else {
            "X"
        }
    }
}

fn score(case: &TestCase, raw_output: &str, plan: &[PlanStep]) -> CaseScore {
    let non_join: Vec<&PlanStep> = plan.iter().filter(|s| !s.is_join()).collect();
    let observed: Vec<String> = non_join.iter().map(|s| s.name.clone()).collect();
    let observed_set: BTreeSet<&str> = observed.iter().map(String::as_str).collect();

    // 1. Syntactic validity
    let has_terminator = plan.iter().any(|s| s.is_join());
    let valid_syntax = if case.allow_abstain {
        (non_join.is_empty() && has_terminator) || !non_join.is_empty()
    } else {
        !plan.is_empty() && (has_terminator || !non_join.is_empty())
    };

    // 2. Tool selection
    let expected_set: BTreeSet<&str> = case.expected_tools.iter().copied().collect();
    let mut accepted_sets: Vec<BTreeSet<&str>> = vec![expected_set];
    for alt in case.acceptable_alts {
        accepted_sets.push(alt.iter().copied().collect());
    }
    let tool_selection_ok = accepted_sets.iter().any(|s| s == &observed_set);

    // 3. Arg shape — arity check per non-join call
    let mut arg_shape_ok = true;
    for call in &non_join {
        if let Some(expected_arity) = tool_arity(&call.name) {
            if call.args.len() != expected_arity {
                arg_shape_ok = false;
            }
        }
    }

    // 4. Hallucinated tool — any non-join call whose name isn't a Lupus tool
    let hallucinated = non_join
        .iter()
        .any(|c| !LUPUS_TOOL_NAMES.contains(&c.name.as_str()));

    // 5. Multi-step correctness
    let (multi_step_ok, dependency_ok) = if case.multi_step {
        let two_or_more = non_join.len() >= 2;
        let dep_ok = if case.requires_dependency {
            non_join.iter().any(|c| !c.references.is_empty())
        } else {
            true
        };
        (two_or_more, dep_ok)
    } else {
        (true, true)
    };

    // 6. Abstention correctness
    let abstained_correctly = if case.allow_abstain {
        non_join.is_empty()
    } else {
        true
    };

    CaseScore {
        id: case.id,
        query: case.query.to_string(),
        raw_output: raw_output.to_string(),
        observed_tools: observed,
        valid_syntax,
        tool_selection_ok,
        arg_shape_ok,
        hallucinated,
        multi_step_ok,
        abstained_correctly,
        dependency_ok,
    }
}

fn color_threshold(name: &str, value: f64) -> &'static str {
    match name {
        "tool_selection" => {
            if value >= 0.80 { "GREEN" } else if value >= 0.60 { "YELLOW" } else { "RED" }
        }
        "arg_shape" => {
            if value >= 0.90 { "GREEN" } else if value >= 0.75 { "YELLOW" } else { "RED" }
        }
        "hallucination" => {
            if value <= 0.02 { "GREEN" } else if value <= 0.05 { "YELLOW" } else { "RED" }
        }
        "multi_step" => {
            if value >= 0.60 { "GREEN" } else if value >= 0.40 { "YELLOW" } else { "RED" }
        }
        "abstention" => {
            if value >= 0.75 { "GREEN" } else if value >= 0.50 { "YELLOW" } else { "RED" }
        }
        "syntactic_validity" => {
            if value >= 0.90 { "GREEN" } else if value >= 0.70 { "YELLOW" } else { "RED" }
        }
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .ok_or("CARGO_MANIFEST_DIR has no parent")?;

    let model_path = repo_root.join("dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf");
    let lora_path = repo_root.join("dist/lupus-tinyagent-search/adapter.gguf");

    println!("Loading model + LoRA...");
    let load_start = Instant::now();
    let mut engine = InferenceEngine::load(&model_path, &lora_path)?;
    println!("  loaded in {:.1}s\n", load_start.elapsed().as_secs_f64());

    let system_prompt = planner_system_prompt();
    let total = TEST_CASES.len();
    let mut scores: Vec<CaseScore> = Vec::with_capacity(total);
    let mut cumulative = 0.0_f64;

    println!(
        "{:>3}  {:<4}  {:<28}  {:<32}  query",
        "id", "mark", "expected", "observed"
    );
    println!("{}", "-".repeat(120));

    for case in TEST_CASES {
        let user_prompt = format!("Question: {}", case.query);
        let t0 = Instant::now();
        let raw =
            engine.infer_blocking(system_prompt, &user_prompt, MAX_PLANNER_TOKENS, true)?;
        let elapsed = t0.elapsed().as_secs_f64();
        cumulative += elapsed;

        let plan = plan::parse_plan(&raw)?;
        let s = score(case, &raw, &plan);

        let expected_str = if case.expected_tools.is_empty() {
            "(abstain)".to_string()
        } else {
            case.expected_tools.join(",")
        };
        let observed_str = if s.observed_tools.is_empty() {
            "(none)".to_string()
        } else {
            s.observed_tools.join(",")
        };
        let query_short: String = case.query.chars().take(50).collect();
        println!(
            "{:>3}  {:<4}  {:<28}  {:<32}  {}",
            case.id,
            s.marker(),
            expected_str,
            observed_str,
            query_short
        );
        scores.push(s);
    }

    println!();
    println!("Total inference time: {:.1}s", cumulative);

    // Aggregate metrics
    let n = scores.len() as f64;
    let validity = scores.iter().filter(|s| s.valid_syntax).count() as f64 / n;
    let selection = scores.iter().filter(|s| s.tool_selection_ok).count() as f64 / n;
    let arg_shape = scores.iter().filter(|s| s.arg_shape_ok).count() as f64 / n;
    let hallucination = scores.iter().filter(|s| s.hallucinated).count() as f64 / n;

    let multi_indices: Vec<usize> = TEST_CASES
        .iter()
        .enumerate()
        .filter_map(|(i, c)| if c.multi_step { Some(i) } else { None })
        .collect();
    let multi_step = if multi_indices.is_empty() {
        1.0
    } else {
        multi_indices
            .iter()
            .filter(|&&i| scores[i].multi_step_ok && scores[i].dependency_ok)
            .count() as f64
            / multi_indices.len() as f64
    };

    let abstain_indices: Vec<usize> = TEST_CASES
        .iter()
        .enumerate()
        .filter_map(|(i, c)| if c.allow_abstain { Some(i) } else { None })
        .collect();
    let abstention = if abstain_indices.is_empty() {
        1.0
    } else {
        abstain_indices
            .iter()
            .filter(|&&i| scores[i].abstained_correctly)
            .count() as f64
            / abstain_indices.len() as f64
    };

    println!();
    println!("==============================================================================");
    println!("Phase 7 daemon parity metrics");
    println!("==============================================================================");
    println!(
        "  Syntactic validity rate    {:6.1}%  [{}]",
        validity * 100.0,
        color_threshold("syntactic_validity", validity)
    );
    println!(
        "  Tool selection accuracy    {:6.1}%  [{}]",
        selection * 100.0,
        color_threshold("tool_selection", selection)
    );
    println!(
        "  Argument shape validity    {:6.1}%  [{}]",
        arg_shape * 100.0,
        color_threshold("arg_shape", arg_shape)
    );
    println!(
        "  Hallucinated tool rate     {:6.1}%  [{}]",
        hallucination * 100.0,
        color_threshold("hallucination", hallucination)
    );
    println!(
        "  Multi-step correctness     {:6.1}%  [{}]  ({} cases)",
        multi_step * 100.0,
        color_threshold("multi_step", multi_step),
        multi_indices.len()
    );
    println!(
        "  Abstention correctness     {:6.1}%  [{}]  ({} cases)",
        abstention * 100.0,
        color_threshold("abstention", abstention),
        abstain_indices.len()
    );
    println!();

    let ok_count = scores.iter().filter(|s| s.marker() == "ok").count();
    println!("Hard pass (ok): {}/{}", ok_count, total);
    println!();

    println!("Python eval reference (eval/tinyagent_runs/20260408-111025-eval.jsonl):");
    println!("  Hard pass: 21/22 | Tool selection: 95.5% GREEN | Hallucination: 0.0% GREEN");
    println!();

    // Acceptance gate: matches the Phase 7 success criterion in the
    // integration plan. We require AT LEAST the prompt-only baseline
    // (selection >= 80%, hallucination <= 5%) — ideally we hit the LoRA
    // numbers (95% / 0%) but the daemon's behavior is allowed to vary
    // slightly from the Python eval because of context-management
    // differences (Python reuses one context, daemon creates fresh ones).
    let pass = selection >= 0.80 && hallucination <= 0.05;
    if pass {
        println!("ACCEPT: daemon clears the Phase 7 acceptance gate (selection >= 80%, halluc <= 5%)");
        Ok(())
    } else {
        println!("FAIL: daemon does NOT clear the Phase 7 gate");
        println!("  required: selection >= 80%, hallucination <= 5%");
        println!(
            "  actual:   selection {:.1}%, hallucination {:.1}%",
            selection * 100.0,
            hallucination * 100.0
        );
        std::process::exit(2);
    }
}

// Suppress dead-code warnings on PlanArg variants — we use them
// indirectly through PlanStep but don't pattern-match them in this file.
#[allow(dead_code)]
fn _silence_plan_arg_warning(_a: &PlanArg) {}
