//! Lupus planner system prompt — Rust port of the canonical Python
//! implementation in `tools/tinyagent_prompt_probe.py`.
//!
//! This module renders the LLMCompiler-style system prompt that the
//! TinyAgent planner LoRA was trained against. The trained adapter is
//! sensitive to the exact prompt: a single byte of drift can degrade
//! tool selection from 95.5% (eval/tinyagent_runs/20260408-111025-eval.jsonl)
//! back to the prompt-only baseline of 72.7%, with no observable error
//! at runtime. The unit test at the bottom of this file pins the SHA-256
//! of the rendered prompt to the value emitted by the Python reference
//! and fails the build on any drift. **Do not refactor or "clean up"
//! the constants in this module.** Port verbatim, refactor never.
//!
//! When iterating on the Python prompt intentionally, regenerate the
//! reference hash with `python tools/dump_planner_prompt.py --hash`
//! and update [`CANONICAL_PROMPT_SHA256`] in the test below alongside
//! whatever change you made to the constants here. Both halves of the
//! change land in one commit, both halves are reviewed together.

use std::sync::OnceLock;

/// LLMCompiler termination sentinel. The trained LoRA emits this after
/// `join()` to signal end-of-plan; llama.cpp's stop-token list trims it
/// from generated output, but it appears verbatim in the in-context
/// examples below so the planner learns to emit it.
pub const END_OF_PLAN: &str = "<END_OF_PLAN>";

// ---------------------------------------------------------------------------
// Lupus tool surface — verbatim port of LUPUS_TOOLS in
// tools/tinyagent_prompt_probe.py:57-117.
//
// Each constant is the multiline TinyAgent-native description for one tool.
// The order is significant — it determines the numbered position the planner
// learns to associate with each tool.
// ---------------------------------------------------------------------------

const TOOL_DESC_SEARCH_SUBNET: &str = "search_subnet(query: str, scope: str) -> dict\n - Search the cooperative subnet for matching datapod metadata.\n - 'query' is the search query string.\n - 'scope' is an optional subnet scope (e.g. 'hvym'); use an empty string if not applicable.\n - Returns a dict with a 'matches' list of datapod entries.\n";

const TOOL_DESC_SEARCH_LOCAL_INDEX: &str = "search_local_index(query: str, top_k: int) -> dict\n - Search the local semantic index for previously visited pages.\n - 'query' is the search query string.\n - 'top_k' is the maximum number of results; use 10 if not specified.\n - Returns a dict with a 'results' list of (url, title, summary, score) entries.\n";

const TOOL_DESC_FETCH_PAGE: &str = "fetch_page(url: str) -> dict\n - Fetch page content by URL.\n - Supports both hvym:// datapod URLs and https:// URLs.\n - Returns a dict with 'url', 'content_type', 'body', and 'status'.\n";

const TOOL_DESC_EXTRACT_CONTENT: &str = "extract_content(html: str, format: str) -> dict\n - Extract clean text, title, summary, and keywords from raw HTML.\n - 'html' is the raw HTML string to extract from. You MUST always pass it.\n - 'format' is either \"full\" or \"summary\". You MUST always pass it; default to \"full\" if the user did not specify.\n - Returns a dict with 'title', 'summary', 'content', 'keywords', and 'content_type'.\n - This tool can only be used AFTER calling fetch_page to get the HTML body.\n";

const TOOL_DESC_SCAN_SECURITY: &str = "scan_security(html: str, url: str) -> dict\n - Scan an HTML page and its URL for security threats.\n - 'html' is the raw HTML body of the page.\n - 'url' is the URL of the page being scanned.\n - Returns a dict with a 'score' (0-100, higher is safer) and a 'threats' list.\n";

const TOOL_DESC_CRAWL_INDEX: &str = "crawl_index(source: str) -> dict\n - Fetch content by CID or URL and create a local index entry for it.\n - 'source' is either an IPFS CID or an https:// URL.\n - Returns a dict with 'indexed', 'url', and 'title'.\n";

/// All 6 Lupus tool descriptions in their numbered order. Position N in
/// this slice maps to step N+1 in the planner's plan output.
const TOOL_DESCRIPTIONS: [&str; 6] = [
    TOOL_DESC_SEARCH_SUBNET,
    TOOL_DESC_SEARCH_LOCAL_INDEX,
    TOOL_DESC_FETCH_PAGE,
    TOOL_DESC_EXTRACT_CONTENT,
    TOOL_DESC_SCAN_SECURITY,
    TOOL_DESC_CRAWL_INDEX,
];

// ---------------------------------------------------------------------------
// LLMCompiler scaffold sections — verbatim from
// tools/tinyagent_prompt_probe.py:121-128 (JOIN_DESCRIPTION),
// 268-275 (LUPUS_CUSTOM_INSTRUCTIONS), and 295-309 (the GUIDELINES block
// inside build_planner_system_prompt).
// ---------------------------------------------------------------------------

const JOIN_DESCRIPTION: &str = "join():\n - Collects and combines results from prior actions.\n - A LLM agent is called upon invoking join to either finalize the user query or wait until the plans are executed.\n - join should always be the last action in the plan, and will be called in two scenarios:\n   (a) if the answer can be determined by gathering the outputs from tasks to generate the final response.\n   (b) if the answer cannot be determined in the planning phase before you execute the plans. ";

const GUIDELINES: &str = "Guidelines:\n - Each action described above contains input/output types and description.\n    - You must strictly adhere to the input and output types for each action.\n    - The action descriptions contain the guidelines. You MUST strictly follow those guidelines when you use the actions.\n - Each action in the plan should strictly be one of the above types. Follow the Python conventions for each action.\n - Each action MUST have a unique ID, which is strictly increasing.\n - Inputs for actions can either be constants or outputs from preceding actions. In the latter case, use the format $id to denote the ID of the previous action whose output will be the input.\n - Always call join as the last action in the plan. Say '<END_OF_PLAN>' after you call join\n - Ensure the plan maximizes parallelizability.\n - Only use the provided action types. If a query cannot be addressed using these, invoke the join action for the next steps.\n - Never explain the plan with comments (e.g. #).\n - Never introduce new actions other than the ones provided.\n\n";

const LUPUS_CUSTOM_INSTRUCTIONS: &str = " - You need to start your plan with the '1.' call\n - Do not use named arguments in your tool calls.\n - You MUST end your plans with the 'join()' call and a '\\n' character.\n - You MUST fill every argument in the tool calls, even if they are optional.\n - If you want to use the result of a previous tool call, you MUST use the '$' sign followed by the index of the tool call.\n - You MUST ONLY USE join() at the very very end of the plan, or you WILL BE PENALIZED.\n";

// ---------------------------------------------------------------------------
// In-context examples — verbatim from LUPUS_EXAMPLES in
// tools/tinyagent_prompt_probe.py:155-235.
//
// For v1 the daemon always exposes the full Lupus tool surface, so we always
// emit all 7 examples (no per-query filtering). When ToolRAG is revisited
// in a future iteration, this static block will need to grow a per-tool-set
// filter mirroring `build_in_context_examples` in the Python source. The
// `is_full_surface` branch in the Python builder always evaluates to True
// for our v1 daemon path.
// ---------------------------------------------------------------------------

const IN_CONTEXT_EXAMPLES: &str = concat!(
    // 1. single tool: search_local_index ("local index" wording)
    "Question: Find pages in my local index about wolves.\n",
    "1. search_local_index(\"wolves\", 10)\n",
    "Thought: I have searched the local index.\n",
    "2. join()<END_OF_PLAN>\n",
    "###\n",
    // 2. single tool: search_subnet (the "datapods" wording cue)
    "Question: Find datapods about decentralized art.\n",
    "1. search_subnet(\"decentralized art\", \"\")\n",
    "Thought: I have searched the cooperative subnet for matching datapods.\n",
    "2. join()<END_OF_PLAN>\n",
    "###\n",
    // 3. single tool: crawl_index (the "add to my index" wording cue)
    "Question: Add https://wikipedia.org/wiki/Wolf to my index.\n",
    "1. crawl_index(\"https://wikipedia.org/wiki/Wolf\")\n",
    "Thought: I have indexed the page.\n",
    "2. join()<END_OF_PLAN>\n",
    "###\n",
    // 4. multi-step chain: fetch_page -> scan_security ("is X safe?" cue)
    "Question: Is https://example.org/login.php safe?\n",
    "1. fetch_page(\"https://example.org/login.php\")\n",
    "2. scan_security(\"$1\", \"https://example.org/login.php\")\n",
    "Thought: I have fetched the page and scanned it for security threats.\n",
    "3. join()<END_OF_PLAN>\n",
    "###\n",
    // 5. multi-step chain: search_subnet -> crawl_index ("find and save" pattern)
    "Question: Find a datapod about felting and save it to my index.\n",
    "1. search_subnet(\"felting\", \"\")\n",
    "2. crawl_index(\"$1\")\n",
    "Thought: I have found the datapod and indexed it.\n",
    "3. join()<END_OF_PLAN>\n",
    "###\n",
    // 6. multi-step chain: search_local_index -> fetch_page ($N reference)
    "Question: Look up wolves in my local index, then fetch the first result in full.\n",
    "1. search_local_index(\"wolves\", 10)\n",
    "2. fetch_page(\"$1\")\n",
    "Thought: I have searched the local index and fetched the first result.\n",
    "3. join()<END_OF_PLAN>\n",
    "###\n",
    // 7. abstention: off-surface task with no possible tool match
    "Question: Translate this French sentence to German: bonjour le monde.\n",
    "Thought: There is no tool available for language translation, so I cannot complete this request.\n",
    "1. join()<END_OF_PLAN>\n",
    "###\n",
);

// ---------------------------------------------------------------------------
// Builder — verbatim port of build_planner_system_prompt() in
// tools/tinyagent_prompt_probe.py:278-323.
// ---------------------------------------------------------------------------

/// Build the rendered planner system prompt by concatenating the scaffold
/// sections in the same order as the Python reference.
///
/// Returns a freshly-allocated `String`. Production code should call
/// [`planner_system_prompt`] instead, which caches the result in a
/// process-wide [`OnceLock`] so the prompt is only built once.
///
/// The byte layout produced here must match the output of the Python
/// `build_planner_system_prompt(LUPUS_TOOLS)` byte-for-byte. The unit
/// test [`tests::canonical_prompt_hash_matches_python`] enforces this
/// via SHA-256 comparison and is the hard guardrail against drift.
fn build_prompt() -> String {
    let n_tools = TOOL_DESCRIPTIONS.len();
    let mut out = String::with_capacity(6000);

    // 1. Header — Python f-string at lines 286-289 of the source.
    out.push_str(
        "Given a user query, create a plan to solve it with the utmost parallelizability. ",
    );
    out.push_str(&format!(
        "Each plan should comprise an action from the following {} types:\n",
        n_tools + 1
    ));

    // 2. Numbered tool descriptions, then join() at position N+1.
    //    Python loops `for i, tool in enumerate(tools)` and appends
    //    `f"{i + 1}. {tool.description}\n"`. The descriptions already end
    //    in `\n`, so the f-string's trailing `\n` produces a blank line
    //    between tools. We mirror this exactly.
    for (i, desc) in TOOL_DESCRIPTIONS.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, desc));
    }
    out.push_str(&format!("{}. {}\n\n", n_tools + 1, JOIN_DESCRIPTION));

    // 3. Guidelines block.
    out.push_str(GUIDELINES);

    // 4. Custom instructions, then a trailing newline.
    out.push_str(LUPUS_CUSTOM_INSTRUCTIONS);
    out.push('\n');

    // 5. Examples header + the rendered example block.
    out.push_str("Here are some examples:\n\n");
    out.push_str(IN_CONTEXT_EXAMPLES);

    out
}

/// Get the canonical Lupus planner system prompt, building it on first
/// call and caching it for the lifetime of the process.
///
/// This is the function the agent loop calls when constructing planner
/// chat messages. It is byte-equivalent to
/// `tools/tinyagent_prompt_probe.py::build_planner_system_prompt(LUPUS_TOOLS)`
/// in the Python eval — that equivalence is the trained LoRA's contract
/// and is enforced by the snapshot test in this file.
pub fn planner_system_prompt() -> &'static str {
    static PROMPT: OnceLock<String> = OnceLock::new();
    PROMPT.get_or_init(build_prompt)
}

// ---------------------------------------------------------------------------
// Snapshot test — the hard guardrail.
//
// Pin the SHA-256 of the rendered prompt to the value emitted by the
// Python reference. Any byte-level drift between the two implementations
// fails this test, which means the LoRA training contract is broken.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// SHA-256 of the canonical Python `build_planner_system_prompt(LUPUS_TOOLS)`
    /// output as of the Step C trained LoRA at commit 1558f82.
    ///
    /// Regenerate with: `python tools/dump_planner_prompt.py --hash`
    ///
    /// **If this hash changes, the trained LoRA's prompt contract is
    /// broken and the daemon's planner output will degrade.** Update
    /// this constant only when you have intentionally changed the
    /// Python prompt AND retrained / re-evaluated the LoRA against the
    /// new prompt.
    const CANONICAL_PROMPT_SHA256: &str =
        "373b83e0d90e1c56bd0ae9daa22f7a1b5c0a6a37d1a790c7895387b3fb49127f";

    /// Expected length in bytes of the rendered prompt. Cross-check
    /// against the SHA-256 — if length matches but hash doesn't, the
    /// drift is content; if length differs, the drift is structural.
    const CANONICAL_PROMPT_LEN: usize = 5303;

    #[test]
    fn canonical_prompt_hash_matches_python() {
        let prompt = planner_system_prompt();

        // Length check first — gives a clearer failure message than the
        // hash check when there's a structural drift like a missing
        // section or an extra newline.
        assert_eq!(
            prompt.len(),
            CANONICAL_PROMPT_LEN,
            "rendered prompt length {} does not match canonical Python length {}. \
             Run `python tools/dump_planner_prompt.py --stats` to compare.",
            prompt.len(),
            CANONICAL_PROMPT_LEN,
        );

        let mut hasher = Sha256::new();
        hasher.update(prompt.as_bytes());
        let actual = hex_lower(&hasher.finalize());

        assert_eq!(
            actual, CANONICAL_PROMPT_SHA256,
            "rendered prompt SHA-256 {} does not match canonical Python {}. \
             The trained planner LoRA contract is broken — silent regression \
             from 95.5% tool selection back to ~72%. \
             See daemon/src/agent/prompt.rs comments for the recovery procedure.",
            actual, CANONICAL_PROMPT_SHA256,
        );
    }

    #[test]
    fn prompt_starts_with_expected_header() {
        let prompt = planner_system_prompt();
        assert!(
            prompt.starts_with("Given a user query, create a plan to solve it"),
            "prompt header drifted; got first 80 chars: {:?}",
            &prompt[..80.min(prompt.len())]
        );
    }

    #[test]
    fn prompt_ends_with_translation_abstention_example() {
        let prompt = planner_system_prompt();
        assert!(
            prompt.ends_with("1. join()<END_OF_PLAN>\n###\n"),
            "prompt tail drifted; got last 80 chars: {:?}",
            &prompt[prompt.len().saturating_sub(80)..]
        );
    }

    #[test]
    fn planner_system_prompt_is_cached() {
        // Calling twice should return the same `&'static str` (same pointer
        // identity), proving the OnceLock cache works.
        let p1 = planner_system_prompt();
        let p2 = planner_system_prompt();
        assert_eq!(p1.as_ptr(), p2.as_ptr());
    }

    /// Convert a byte slice to a lowercase hex string. We avoid pulling
    /// in the `hex` crate just for this one call.
    fn hex_lower(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}
