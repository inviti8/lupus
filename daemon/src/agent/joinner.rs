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
/// `Action: Finish(...)` outputs. Verbatim from the upstream Python
/// reference. Used as the system message for joinner inference calls.
pub const OUTPUT_PROMPT_FINAL: &str = concat!(
    "Follow these rules:\n",
    " - You MUST only output Finish, or you WILL BE PENALIZED.\n",
    " - If you need to answer some knowledge question, just answer it directly using 'Action: Finish(<your answer>)'.\n",
    " - If you need to return the result of a summary (summarize_pdf), you MUST use 'Action: Finish(Summary)'\n",
    " - If there is an error in one of the tool calls and it is not fixable, you should provide a user-friendly error message using 'Action: Finish(<your error message>)'.\n",
    "\n",
    "Here are some examples:\n",
    "Question: Create a zoom meeting for the upcoming Apple meeting with Eren Erdoğan. \n",
    "get_email_address(\"Eren Erdoğan\")\n",
    "Observation: eren@gmail.com\n",
    "get_zoom_meeting_link(\"Apple Meeting\", \"2022-10-14 15:00:00\", 60, [\"$1\"])\n",
    "Observation: https://zoom.us/j/1234567890?pwd=abc123\n",
    "create_calendar_event(\"Apple Meeting\", \"2022-10-14 15:00:00\", \"2022-10-14 16:00:00\", \"Apple HQ\", \"$2\", None)\n",
    "Observation: Event created successfully\n",
    "Thought: I don't need to answer a question.\n",
    "Action: Finish(Task completed!)\n",
    "###\n",
    "Question: What is the content of the Apple meeting notes? \n",
    "get_note_content(\"Apple Meeting\")\n",
    "Observation: The meeting is about the new iPhone release.\n",
    "Thought: I can just answer the question directly.\n",
    "Action: Finish(The meeting is about the new iPhone release.)\n",
    "###\n",
    "Question: Compose a new email to John, attaching the Project.pdf file.\n",
    "get_email_address(\"John\")\n",
    "Observation: john@doe.comopen_and_get_file_path(\"Project\")\n",
    "Observation: /Users/eren/Downloads/Project.pdf\n",
    "compose_new_email([john@doe.com], [], \"Project Update\", \"Please find the attached project update.\", [\"/Users/eren/Downloads/Project.pdf\"])\n",
    "Observation: There was an error while composing the email.\n",
    "Thought: There was an error with the compose_new_email tool call and it is not possible to fix it. I need to provide a user-friendly error message.\n",
    "Action: Finish(There was an error while composing the email. Please try again later.)\n",
    "###\n",
    "Question: Summarize the Apple Demo file. \n",
    "open_and_get_file_path(Apple Demo)\n",
    "Observation: /Users/eren/Downloads/Apple_Demo.pdf\n",
    "summarize_pdf(/Users/eren/Downloads/Apple_Demo.pdf)\n",
    "Observation: The new iPhone is going to be released in 2023.\n",
    "Action: Finish(Summary)\n",
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
    // The joinner sees a single combined "user" message because TinyAgent's
    // GGUF chat template is flat-concat (no role markers). The system
    // prompt + user message are concatenated by the InferenceEngine's
    // manual chat template path. The Python upstream version also
    // builds this as a single prompt string before calling the LLM.
    // End the prompt with "Thought:" so the model knows to continue
    // generating in the expected format. Without this continuation
    // prompt, TinyAgent-1.1B emits EOS immediately after the scratchpad
    // — it interprets the end-of-scratchpad as end-of-conversation.
    // The Python reference produces the same effect via the chat
    // template's assistant turn prefix.
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
/// If no action line is found, returns an empty answer (the caller can
/// detect this as a malformed joinner response and surface an error).
pub fn parse_joinner_output(raw: &str) -> JoinnerOutput {
    let mut thought = String::new();
    let mut answer = String::new();
    let mut is_replan = false;

    for raw_line in raw.split('\n') {
        // Find "Action:" anywhere in the line; if present, work from
        // that point onward (matches the Python `start_of_answer` logic).
        let line = if let Some(start) = raw_line.find("Action:") {
            &raw_line[start..]
        } else {
            raw_line
        };

        if line.starts_with("Action:") || line.starts_with(" Answer:") {
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
        assert!(OUTPUT_PROMPT_FINAL.contains("You MUST only output Finish"));
        assert!(OUTPUT_PROMPT_FINAL.contains("Action: Finish"));
        // The four FINISH_EXAMPLES should all appear, separated by ###\n
        let example_count = OUTPUT_PROMPT_FINAL.matches("###\n").count();
        assert_eq!(example_count, 4, "expected 4 example separators");
    }
}
