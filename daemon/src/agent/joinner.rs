//! Joinner second-pass — converts an executed LLMCompiler plan into a
//! natural-language `Action: Finish(<answer>)` reply for the user.
//!
//! This is the second of the two LLM calls per search query. After the
//! planner produces a plan and the executor runs it, the joinner sees
//! the (query, executed plan + observations) and synthesizes a final
//! answer in natural language.
//!
//! ## Why we run the joinner with NO LoRA
//!
//! The trained search LoRA was fine-tuned exclusively on planner data —
//! 354 (query, LLMCompiler plan) pairs. It teaches the model to emit
//! `1. tool(args)\n2. join()<END_OF_PLAN>` style output. If we attach
//! that LoRA to the joinner call, the model is biased toward emitting
//! another LLMCompiler plan instead of an `Action: Finish(...)` response.
//!
//! BAIR's base TinyAgent model already knows the joinner output format
//! from its original training. Detaching the LoRA for joinner calls
//! gives us the unmodified base behavior. See the `use_lora: false`
//! parameter in [`InferenceEngine::infer_blocking`].
//!
//! ## Prompt structure
//!
//! Mirrors `dist/tinyagent-source/src/llm_compiler/llm_compiler.py:217-222`:
//!
//! ```text
//! {OUTPUT_PROMPT_FINAL}
//! Question: {input_query}
//!
//! {agent_scratchpad}
//! ```
//!
//! Where `agent_scratchpad` is the rendered list of executed steps:
//!
//! ```text
//! 1. tool_one("arg")
//! Observation: {result_json}
//! 2. tool_two("$1")
//! Observation: {result_json}
//! ```
//!
//! See [`build_scratchpad`].

use crate::agent::executor::ExecutionRecord;
use crate::agent::inference::InferenceEngine;
use crate::error::LupusError;

/// Maximum chars from a single observation when rendering the
/// scratchpad. Tool observations can be large (e.g. `fetch_page` body
/// fields with full HTML); we truncate to keep the joinner's context
/// budget under control. The base model has a 32K context but we use
/// 4K (`DEFAULT_N_CTX`).
const MAX_OBSERVATION_CHARS: usize = 800;

/// Maximum tokens to generate per joinner call. The Finish payload is
/// usually 1-2 sentences (~20-80 tokens). 256 is comfortable headroom
/// without risking runaway generation.
pub const MAX_JOINNER_TOKENS: usize = 256;

// ---------------------------------------------------------------------------
// OUTPUT_PROMPT_FINAL — verbatim port from
// dist/tinyagent-source/src/tiny_agent/prompts.py:207-213.
//
// The Python source builds this string at module-init time by joining
// JOINNER_FINISH_RULES + a "###\n"-separated list of FINISH_EXAMPLES.
// We render it once at compile time as a single &'static str. The
// example tools (compose_new_email, get_zoom_meeting_link, etc.) are
// from BAIR's Apple-app surface — they teach the OUTPUT SHAPE, not the
// tool semantics, so they don't need to mention Lupus tools.
// ---------------------------------------------------------------------------

/// The joinner's "Follow these rules" system prompt with example
/// `Action: Finish(...)` outputs.
///
/// ## History
///
/// v0.1 imported BAIR/TinyAgent's `OUTPUT_PROMPT_FINAL` verbatim from
/// `dist/tinyagent-source/src/tiny_agent/prompts.py`. That prompt was
/// designed for an Apple device agent where a dedicated `summarize_pdf`
/// tool produced summaries and the joinner's job was to announce
/// "Summary" (rule #3: "you MUST use 'Action: Finish(Summary)'"). The
/// eval at `daemon/tests/joinner_golden.rs` showed the 1.1B base
/// parroting that rule verbatim — every summarize query returned the
/// literal string "Summary", every factoid returned "Task completed!".
///
/// v0.2 (this version) rewrites the prompt for Lupus's actual
/// pipeline: `extract_content` produces the raw content, the joinner
/// SYNTHESIZES the final answer from it. Examples use our real tool
/// names (`fetch_page`, `extract_content`) so the in-context learning
/// primes observation shapes we actually produce, not Apple meeting
/// mocks. Voice is first-person AI assistant ("I couldn't extract...").
pub const OUTPUT_PROMPT_FINAL: &str = concat!(
    "You are the second-pass reasoner for a web search agent. Given a user Question and the Observations from the tools the agent ran, produce a final answer wrapped in `Action: Finish(<your answer>)`.\n",
    "\n",
    "Rules:\n",
    " - When the Observations contain usable content (a `summary`, `body`, or similar field with real text), write a grounded 1-3 sentence answer that paraphrases or quotes that content. Do NOT echo the literal word \"Summary\" — write an actual summary.\n",
    " - When the Observations are empty, errored, or contain no usable content, acknowledge that honestly in first-person: \"I couldn't extract a summary from that page\", \"I couldn't reach that page\", etc.\n",
    " - When there are no Observations at all (no tool calls were made), answer the question from your own knowledge.\n",
    " - Always end with a single `Action: Finish(...)` line. You may precede it with one `Thought:` line explaining your reasoning. Do NOT output multiple Action lines or additional examples.\n",
    "\n",
    "Here are some examples:\n",
    "Question: summarize https://en.wikipedia.org/wiki/Wolf\n",
    "fetch_page(\"https://en.wikipedia.org/wiki/Wolf\")\n",
    "Observation: {\"url\":\"https://en.wikipedia.org/wiki/Wolf\",\"http_status\":200,\"body\":\"...\"}\n",
    "extract_content(\"$1\", \"summary\")\n",
    "Observation: {\"title\":\"Wolf - Wikipedia\",\"summary\":\"The wolf (Canis lupus) is a large canine native to Eurasia and North America. More than thirty subspecies of Canis lupus have been recognized.\"}\n",
    "Thought: The extract_content observation contains a usable summary.\n",
    "Action: Finish(The wolf (Canis lupus) is a large canine native to Eurasia and North America, with more than thirty recognized subspecies. It is the largest extant member of the family Canidae.)\n",
    "###\n",
    "Question: summarize https://example.com/blank\n",
    "fetch_page(\"https://example.com/blank\")\n",
    "Observation: {\"url\":\"https://example.com/blank\",\"http_status\":200,\"body\":\"<html></html>\"}\n",
    "extract_content(\"$1\", \"summary\")\n",
    "Observation: {\"title\":\"\",\"summary\":\"\",\"keywords\":[]}\n",
    "Thought: The page returned no extractable content; I cannot summarize what isn't there.\n",
    "Action: Finish(I couldn't extract a summary from that page — the content was empty.)\n",
    "###\n",
    "Question: summarize https://down.example.com\n",
    "fetch_page(\"https://down.example.com\")\n",
    "Observation: Error: tool error [fetch_page]: host fetch failed: DNS lookup failed\n",
    "extract_content(\"$1\", \"summary\")\n",
    "Observation: Error: arg coercion: $1 references step that errored\n",
    "Thought: The fetch tool failed and nothing could be extracted downstream.\n",
    "Action: Finish(I couldn't reach that page — the server was unreachable.)\n",
    "###\n",
    "Question: what is the capital of france\n",
    "Thought: I can answer this from my own knowledge without using tools.\n",
    "Action: Finish(The capital of France is Paris.)\n",
    "###\n",
);

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// Result of a joinner call. The user-facing answer is in `answer`;
/// `is_replan` is set when the model emitted `Action: Replan` instead
/// of `Action: Finish` (which can happen if the joinner ignored our
/// "MUST only output Finish" instruction).
#[derive(Debug, Clone)]
pub struct JoinnerOutput {
    /// The optional `Thought:` line that preceded the action, if any.
    pub thought: String,
    /// The contents of `Finish(<answer>)` or `Replan(<reason>)`,
    /// extracted as the substring between the first `(` and the LAST `)`
    /// on the action line.
    pub answer: String,
    /// True if the model emitted `Action: Replan` rather than
    /// `Action: Finish`. v1 treats Replan as a hard error; v2 will
    /// loop back to the planner with the replan context.
    pub is_replan: bool,
    /// Raw model output before parsing — useful for debugging when the
    /// parser returns an empty answer (the model might have produced
    /// free-form text without the expected `Action:` prefix).
    pub raw_output: String,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the joinner second-pass: builds the agent scratchpad from
/// `records`, calls the inference engine WITHOUT the LoRA, parses the
/// model's `Action: Finish(...)` response.
///
/// Returns the parsed [`JoinnerOutput`]. Per decision D in the daemon
/// integration plan, v1 callers should treat `is_replan = true` as a
/// hard error and surface "joinner requested replan" to the user; the
/// replan loop is deferred to v2.
pub fn run_joinner(
    engine: &mut InferenceEngine,
    user_query: &str,
    records: &[ExecutionRecord],
) -> Result<JoinnerOutput, LupusError> {
    let scratchpad = build_scratchpad(records);
    run_joinner_with_scratchpad(engine, user_query, &scratchpad)
}

/// Call the joinner with a pre-rendered scratchpad string. Bypasses
/// `build_scratchpad` so integration tests can feed synthetic scratchpads
/// and isolate joinner quality from tool-execution quirks. Used by
/// `daemon/tests/joinner_golden.rs`.
pub fn run_joinner_with_scratchpad(
    engine: &mut InferenceEngine,
    user_query: &str,
    scratchpad: &str,
) -> Result<JoinnerOutput, LupusError> {
    // The joinner sees a single combined "user" message because TinyAgent's
    // GGUF chat template is flat-concat (no role markers). The system
    // prompt + user message are concatenated by the InferenceEngine's
    // manual chat template path. The Python upstream version also
    // builds this as a single prompt string before calling the LLM.
    // End the prompt with "Thought:" so the model knows to start
    // generating. Without this continuation cue, TinyAgent-1.1B emits
    // EOS as its very first token — it interprets the
    // end-of-scratchpad as end-of-conversation and terminates
    // immediately, producing empty joinner_raw.
    //
    // With "Thought:" the model generates a thought sentence but
    // often stops without emitting the strict "Action: Finish(...)"
    // wrapper that example 4 of OUTPUT_PROMPT_FINAL shows. That's
    // handled by the lenient fallback in [`parse_joinner_output`] —
    // if no Action line is found but the raw output is non-empty,
    // the trimmed output (or the thought) is used as the answer.
    //
    // This is the minimum hack that makes both halves work:
    //   - model emits text (Thought: cue prevents instant EOS)
    //   - parser surfaces that text (fallback handles missing Action)
    let user_message = format!("Question: {user_query}\n\n{scratchpad}\nThought:");

    let raw = engine.infer_blocking(
        OUTPUT_PROMPT_FINAL,
        &user_message,
        MAX_JOINNER_TOKENS,
        /* use_lora */ false,
        crate::agent::inference::JOINNER_STOP_STRINGS,
    )?;

    let mut parsed = parse_joinner_output(&raw);
    parsed.raw_output = raw;
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Scratchpad builder — port of get_though_action_observation per task
// from upstream task_fetching_unit.py:59-79.
// ---------------------------------------------------------------------------

/// Build the agent scratchpad string that the joinner sees as part of
/// its input. Format matches upstream's `FINISH_EXAMPLES` (in
/// `dist/tinyagent-source/src/tiny_agent/prompts.py:161-191`) which
/// uses **unnumbered** tool calls:
///
/// ```text
/// tool_name("arg1", arg2)
/// Observation: <result>
/// another_tool("$1")
/// Observation: <result>
/// ```
///
/// We deliberately drop the `N.` step numbers here even though the
/// planner emits them. The 1.1B base model is sensitive to format
/// drift between in-context examples and the live prompt — feeding
/// numbered tool calls causes it to interpret the scratchpad as a
/// partial plan and emit `2. another_tool(...)` instead of
/// `Action: Finish(...)`.  Matching FINISH_EXAMPLES exactly is the
/// reliable way to make the joinner output the right format.
///
/// Failed steps render as `Observation: Error: <message>` so the
/// joinner can produce a user-friendly error message per the third
/// JOINNER_FINISH_RULES bullet.
///
/// Observations are JSON-stringified and truncated to
/// [`MAX_OBSERVATION_CHARS`] to keep the prompt within the context
/// budget. Long bodies (e.g. raw HTML from `fetch_page`) get a
/// `... [truncated]` suffix.
pub fn build_scratchpad(records: &[ExecutionRecord]) -> String {
    let mut out = String::with_capacity(512);
    for record in records {
        if record.is_join() {
            // Skip the join terminator — it's not a real tool call and
            // the joinner doesn't need to see it.
            continue;
        }
        // Unnumbered `tool_name(raw_args)` — matches FINISH_EXAMPLES
        // exactly. raw_args may contain unresolved `$N` references; the
        // joinner reads the observation lines below to understand what
        // actually happened.
        out.push_str(&format!(
            "{}({})\n",
            record.step.name, record.step.raw_args
        ));

        // Observation line — the tool's JSON output, or the error.
        out.push_str("Observation: ");
        match (&record.observation, &record.error) {
            (Some(obs), _) => {
                let json = obs.to_string();
                if json.len() > MAX_OBSERVATION_CHARS {
                    out.push_str(&json[..MAX_OBSERVATION_CHARS]);
                    out.push_str(" ... [truncated]");
                } else {
                    out.push_str(&json);
                }
            }
            (None, Some(err)) => {
                out.push_str("Error: ");
                if err.len() > MAX_OBSERVATION_CHARS {
                    out.push_str(&err[..MAX_OBSERVATION_CHARS]);
                    out.push_str(" ... [truncated]");
                } else {
                    out.push_str(err);
                }
            }
            (None, None) => {
                // Defensive: shouldn't happen for non-join steps after
                // the executor runs, but guard against it.
                out.push_str("(no result)");
            }
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Output parser — port of _parse_joinner_output from
// dist/tinyagent-source/src/llm_compiler/llm_compiler.py:147-172.
// ---------------------------------------------------------------------------

/// Parse the raw joinner output into a [`JoinnerOutput`].
///
/// Mirrors the Python `_parse_joinner_output` function:
/// - Walk lines top to bottom
/// - For each line, find `Action:` (use the substring from there)
/// - Lines starting with `Action:` or ` Answer:` extract content
///   between `(` and the LAST `)`, and detect Replan via substring match
/// - Lines starting with `Thought:` or ` Thought:` capture the thought
///
/// ## Lenient fallback
///
/// If the model produced non-empty output but no `Action: Finish(...)`
/// line, we use the trimmed raw output (or the thought, if present) as
/// the answer. This handles the common case where TinyAgent-1.1B's
/// base model (no LoRA) produces a coherent abstention-style thought
/// but forgets to wrap it in the expected `Action:` format. Without
/// this fallback, the user sees `text_answer: null` even when the
/// model said something meaningful. The planner LoRA is strict about
/// format; the base model is not, and the joinner runs WITHOUT the
/// LoRA by design (see module docs).
pub fn parse_joinner_output(raw: &str) -> JoinnerOutput {
    let mut thought = String::new();
    let mut answer = String::new();
    let mut is_replan = false;
    let mut found_action = false;

    for raw_line in raw.split('\n') {
        // Find "Action:" anywhere in the line; if present, work from
        // that point onward (matches the Python `start_of_answer` logic).
        let line = if let Some(start) = raw_line.find("Action:") {
            &raw_line[start..]
        } else {
            raw_line
        };

        if line.starts_with("Action:") || line.starts_with(" Answer:") {
            found_action = true;
            // Extract the substring between the first `(` and the LAST `)`.
            // This matches Python's `ans[ans.find("(") + 1 : ans.rfind(")")]`.
            if let (Some(open), Some(close)) = (line.find('('), line.rfind(')')) {
                if close > open {
                    answer = line[open + 1..close].to_string();
                }
            }
            if line.contains("Replan") {
                is_replan = true;
            }
        } else if line.starts_with("Thought:") || line.starts_with(" Thought:") {
            // Capture everything after "Thought:".
            if let Some(idx) = line.find("Thought:") {
                thought = line[idx + "Thought:".len()..].trim().to_string();
            }
        }
    }

    // Lenient fallback — if the model produced meaningful output but no
    // Action line, surface what it said rather than returning null.
    // Since the prompt ends with "Thought:" (see `run_joinner`), the
    // model's raw output IS the content of the thought even when no
    // explicit "Thought:" line is present in the output. Populate both
    // thought and answer from it so the browser's plan view shows the
    // reasoning AND the user gets a visible text_answer.
    if !found_action {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if thought.is_empty() {
                thought = trimmed.to_string();
            }
            answer = thought.clone();
        }
    }

    JoinnerOutput {
        thought,
        answer,
        is_replan,
        raw_output: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::plan::{PlanArg, PlanStep};
    use serde_json::json;

    fn step(idx: u32, name: &str, raw_args: &str) -> PlanStep {
        PlanStep {
            idx,
            name: name.to_string(),
            raw_args: raw_args.to_string(),
            args: vec![],
            references: vec![],
        }
    }

    fn record(s: PlanStep, observation: serde_json::Value) -> ExecutionRecord {
        ExecutionRecord {
            step: s,
            observation: Some(observation),
            error: None,
        }
    }

    fn record_err(s: PlanStep, err: &str) -> ExecutionRecord {
        ExecutionRecord {
            step: s,
            observation: None,
            error: Some(err.to_string()),
        }
    }

    fn record_join(idx: u32) -> ExecutionRecord {
        ExecutionRecord {
            step: PlanStep {
                idx,
                name: "join".to_string(),
                raw_args: String::new(),
                args: vec![],
                references: vec![],
            },
            observation: None,
            error: None,
        }
    }

    // ---- build_scratchpad ----

    #[test]
    fn scratchpad_single_step() {
        let recs = vec![
            record(
                step(1, "search_local_index", "\"wolves\", 10"),
                json!({"results": [{"url": "https://wiki/wolves"}]}),
            ),
            record_join(2),
        ];
        let pad = build_scratchpad(&recs);
        // Unnumbered tool call (matches FINISH_EXAMPLES format).
        assert!(pad.contains("search_local_index(\"wolves\", 10)\n"));
        // Should NOT include the leading step number.
        assert!(!pad.contains("1. search_local_index"));
        assert!(pad.contains("Observation: "));
        assert!(pad.contains("https://wiki/wolves"));
        // join() should not appear in the scratchpad
        assert!(!pad.contains("join"));
    }

    #[test]
    fn scratchpad_chain_with_dollar_ref() {
        let recs = vec![
            record(
                step(1, "fetch_page", "\"https://example.org\""),
                json!({"url": "https://example.org", "body": "<html>hello</html>"}),
            ),
            record(
                step(2, "scan_security", "\"$1\", \"https://example.org\""),
                json!({"score": 87, "threats": []}),
            ),
            record_join(3),
        ];
        let pad = build_scratchpad(&recs);
        assert!(pad.contains("fetch_page(\"https://example.org\")"));
        assert!(pad.contains("scan_security(\"$1\", \"https://example.org\")"));
        assert!(pad.contains("\"score\":87"));
        // No numbered prefixes
        assert!(!pad.contains("1. fetch_page"));
        assert!(!pad.contains("2. scan_security"));
    }

    #[test]
    fn scratchpad_renders_error() {
        let recs = vec![
            record_err(
                step(1, "compose_email", "\"hi\""),
                "unknown tool",
            ),
            record_join(2),
        ];
        let pad = build_scratchpad(&recs);
        assert!(pad.contains("compose_email(\"hi\")"));
        assert!(pad.contains("Observation: Error: unknown tool"));
    }

    #[test]
    fn scratchpad_truncates_long_observation() {
        let big = "x".repeat(2000);
        let recs = vec![
            record(
                step(1, "fetch_page", "\"https://x\""),
                json!({"body": big}),
            ),
            record_join(2),
        ];
        let pad = build_scratchpad(&recs);
        assert!(pad.contains("[truncated]"));
        // The full 2000 x's shouldn't all be there
        assert!(pad.len() < 2000 + 200);
    }

    #[test]
    fn scratchpad_skips_join_only() {
        let recs = vec![record_join(1)];
        let pad = build_scratchpad(&recs);
        assert!(pad.is_empty());
    }

    // ---- parse_joinner_output ----

    #[test]
    fn parses_finish_action() {
        let raw = "Thought: I have the answer.\nAction: Finish(The capital of France is Paris.)";
        let out = parse_joinner_output(raw);
        assert_eq!(out.thought, "I have the answer.");
        assert_eq!(out.answer, "The capital of France is Paris.");
        assert!(!out.is_replan);
    }

    #[test]
    fn parses_finish_without_thought() {
        let raw = "Action: Finish(2 + 2 equals 4.)";
        let out = parse_joinner_output(raw);
        assert_eq!(out.thought, "");
        assert_eq!(out.answer, "2 + 2 equals 4.");
        assert!(!out.is_replan);
    }

    #[test]
    fn parses_replan_action() {
        let raw = "Thought: The first attempt failed.\nAction: Replan(retry with a different tool)";
        let out = parse_joinner_output(raw);
        assert!(out.is_replan);
        assert_eq!(out.answer, "retry with a different tool");
    }

    #[test]
    fn extracts_to_last_paren() {
        // Answer contains parens — the parser should pick the LAST `)`.
        let raw = "Action: Finish(The score is high (87/100) — page is safe.)";
        let out = parse_joinner_output(raw);
        assert_eq!(out.answer, "The score is high (87/100) — page is safe.");
    }

    #[test]
    fn handles_action_with_leading_text() {
        // Sometimes the model emits text before "Action:" on the same line
        let raw = "  Action: Finish(yes)";
        let out = parse_joinner_output(raw);
        assert_eq!(out.answer, "yes");
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = parse_joinner_output("");
        assert_eq!(out.answer, "");
        assert_eq!(out.thought, "");
        assert!(!out.is_replan);
    }

    // ---- OUTPUT_PROMPT_FINAL sanity ----

    #[test]
    fn output_prompt_final_has_finish_rule() {
        // v0.2 rewrite: structural checks, not text-identity. The prompt's
        // shape is load-bearing (rules + examples + `###\n` separators);
        // the exact wording iterates with eval feedback.
        assert!(OUTPUT_PROMPT_FINAL.contains("Action: Finish"));
        assert!(OUTPUT_PROMPT_FINAL.contains("Rules:"));
        assert!(OUTPUT_PROMPT_FINAL.contains("Here are some examples:"));
        // Explicit guard: we removed the "MUST use Finish(Summary)" rule
        // (imported from BAIR/TinyAgent) because it caused the 1.1B base
        // to parrot the literal string "Summary". Prevent accidental
        // re-introduction.
        assert!(
            !OUTPUT_PROMPT_FINAL.contains("Finish(Summary)"),
            "v0.2 removed the literal Finish(Summary) example — reintroducing it causes prompt-parroting, see joinner_outputs.md history"
        );
        // Four examples, four separators.
        let example_count = OUTPUT_PROMPT_FINAL.matches("###\n").count();
        assert_eq!(example_count, 4, "expected 4 example separators");
    }
}
