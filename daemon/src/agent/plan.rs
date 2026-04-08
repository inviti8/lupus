//! LLMCompiler plan parser — Rust port of `tools/eval_tinyagent.py::parse_plan`
//! and `_parse_args_string`.
//!
//! Takes raw text from the planner LoRA's output and produces a structured
//! `Vec<PlanStep>` describing each numbered tool call. The Python parser is
//! the oracle; the golden-test fixture in `tests/` runs the Rust parser
//! against the 22 model outputs in `eval/tinyagent_runs/20260408-111025-eval.jsonl`
//! and asserts the result matches the parsed structure that Python emitted
//! at the time the LoRA was validated.
//!
//! ## Grammar
//!
//! The trained LoRA's output is a sequence of lines, each looking like:
//!
//! ```text
//! 1. tool_name("string arg", 10, "$1")
//! Thought: optional thought line, ignored by the parser
//! 2. join()
//! ```
//!
//! Specifically:
//!
//! - Each line is matched by [`ACTION_RE`]: `(?:^|[^\w\$])(\d+)\.\s*(\w+)\s*\((.*?)\)`
//!   The leading `(?:^|[^\w\$])` accepts an optional non-word, non-`$` lead-in,
//!   handling continuation tokens like `". 1. tool(...)"` that the model
//!   sometimes emits for the first step.
//! - Args are comma-separated and consist of:
//!   - String literals: `"text"` or `'text'` (no escapes; the BPE tokenizer
//!     never emits backslash-escaped characters in tool args)
//!   - Int literals: `10`, `-3`
//!   - Anything else (rare; falls back to [`PlanArg::Other`])
//! - `$N` references appear inside string literals as `"$1"`. They're
//!   extracted into [`PlanStep::references`] separately via [`ID_REF_RE`].
//! - Parsing stops at the first `join`/`join_finish`/`join_replan` step.
//!
//! ## What this is NOT
//!
//! The grammar deliberately doesn't support Python list/dict literals or
//! escape sequences. Upstream BAIR examples use `["$1"]` lists for SMS
//! recipient lists; none of our 6 Lupus tools take list-typed args. If a
//! future tool needs them, this parser will need to grow accordingly.

use std::sync::OnceLock;

use regex::Regex;

use crate::error::LupusError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One parsed argument from a tool call. Mirrors what
/// `_parse_args_string` produces in the Python eval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanArg {
    /// A string literal — quoted in the source. May contain a `$N`
    /// reference like `"$1"` (the reference is also extracted into
    /// the parent [`PlanStep::references`] field).
    String(String),
    /// An integer literal — most often the `top_k` parameter.
    Int(i64),
    /// Anything we couldn't parse as String or Int. Preserved as the raw
    /// substring so the executor can decide what to do with it. Mirrors
    /// the Python parser's third fallback path that returns the raw
    /// arg string when `ast.literal_eval` fails twice.
    Other(String),
}

/// One step in a parsed LLMCompiler plan. Numbered, named, with parsed
/// args and a list of `$N` references found in the raw arg string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    /// Step number as emitted by the model. Should start at 1 and
    /// increase monotonically, but the parser doesn't enforce that
    /// (the executor does, via dependency resolution).
    pub idx: u32,
    /// Tool name as emitted by the model. May be a hallucinated tool
    /// not in the Lupus surface; the dispatcher's hard validation at
    /// `daemon/src/tools/mod.rs::execute` is the safety net.
    pub name: String,
    /// Original arg string from inside the parentheses, preserved for
    /// logging and dependency resolution. Trimmed.
    pub raw_args: String,
    /// Parsed positional args. Empty for `join()`.
    pub args: Vec<PlanArg>,
    /// `$N` indices found anywhere in `raw_args`, in source order. Used
    /// by the executor to substitute prior step observations into
    /// dependent args at runtime.
    pub references: Vec<u32>,
}

impl PlanStep {
    /// Return true if this step is a plan terminator (`join`,
    /// `join_finish`, or `join_replan`). The parser stops after the
    /// first join-named step is encountered.
    pub fn is_join(&self) -> bool {
        matches!(self.name.as_str(), "join" | "join_finish" | "join_replan")
    }
}

// ---------------------------------------------------------------------------
// Regexes — verbatim from `tools/eval_tinyagent.py`.
// ---------------------------------------------------------------------------

/// Action regex: matches a single LLMCompiler step on one line.
///
/// Mirrors `tools/eval_tinyagent.py::ACTION_RE`. The leading
/// `(?:^|[^\w\$])` accepts an optional non-word, non-`$` lead-in for
/// the model's continuation-token quirk (e.g. `". 1. search_local_index(...)"`).
/// The trailing `(?:<END_OF_PLAN>)?\s*(?:#.*)?\s*$` allows the optional
/// sentinel and optional comment that some training-data examples carry.
fn action_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?:^|[^\w\$])(\d+)\.\s*(\w+)\s*\((.*?)\)\s*(?:<END_OF_PLAN>)?\s*(?:#.*)?\s*$",
        )
        .expect("ACTION_RE is a literal valid regex")
    })
}

/// `$N` or `${N}` reference regex. Mirrors `tools/eval_tinyagent.py::ID_REF_RE`.
fn id_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{?(\d+)\}?").expect("ID_REF_RE is a literal valid regex"))
}

// ---------------------------------------------------------------------------
// Parser entry point
// ---------------------------------------------------------------------------

/// Parse the raw text output from the planner LoRA into a list of
/// `PlanStep`s.
///
/// Iterates over `text.lines()`, attempts to match each line against the
/// LLMCompiler action regex, accumulates matches into a Vec, and stops at
/// the first `join`-family step. Lines that don't match the regex
/// (e.g. `Thought: ...` lines, blank lines, the leading continuation
/// token line) are silently skipped — this matches the Python parser's
/// `for line in text.splitlines(): m = ACTION_RE.search(line); if not m:
/// continue` behavior.
///
/// Returns an empty `Vec` if no steps were found at all. Returns a `Vec`
/// containing only a join step for the abstention case
/// (`Thought: ... cannot complete this request.\n1. join()`).
///
/// # Errors
///
/// Currently never returns an error — malformed inputs produce an empty
/// `Vec` rather than an error. The function returns `Result` so future
/// stricter parsing can fail without breaking the API.
pub fn parse_plan(text: &str) -> Result<Vec<PlanStep>, LupusError> {
    let mut steps = Vec::new();
    for line in text.lines() {
        let Some(caps) = action_re().captures(line) else {
            continue;
        };
        // Group 1: step index. Group 2: tool name. Group 3: raw args.
        // The regex is constructed so all three are guaranteed to match.
        let idx_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let name = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
        let raw_args = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();

        let idx: u32 = match idx_str.parse() {
            Ok(n) => n,
            Err(_) => continue, // shouldn't happen since regex matches \d+
        };

        let args = parse_args_string(&raw_args);
        let references = id_ref_re()
            .captures_iter(&raw_args)
            .filter_map(|c| c.get(1).and_then(|m| m.as_str().parse::<u32>().ok()))
            .collect();

        let is_join = matches!(name.as_str(), "join" | "join_finish" | "join_replan");
        steps.push(PlanStep {
            idx,
            name,
            raw_args,
            args,
            references,
        });

        if is_join {
            break;
        }
    }
    Ok(steps)
}

// ---------------------------------------------------------------------------
// Hand-rolled positional arg parser
// ---------------------------------------------------------------------------

/// Parse the comma-separated arg string of a single tool call into a
/// list of [`PlanArg`]s.
///
/// Mirrors `tools/eval_tinyagent.py::_parse_args_string` but in Rust we
/// can't use `ast.literal_eval`, so we hand-roll a small tokenizer for
/// the subset of Python literals that actually appears in our model
/// outputs (verified against the 22 cases in `eval/tinyagent_runs/`):
///
/// - String literals: `"text"` or `'text'` (no escape sequences)
/// - Int literals: `10`, `-3`
/// - Anything else falls into [`PlanArg::Other`] preserving the raw text
///
/// Whitespace and commas between args are skipped. Returns an empty
/// `Vec` for an empty or whitespace-only input.
fn parse_args_string(raw: &str) -> Vec<PlanArg> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut args = Vec::new();
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    let len = bytes.len();

    while i < len {
        // Skip leading whitespace and commas before the next arg.
        while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= len {
            break;
        }

        let c = bytes[i];

        if c == b'"' || c == b'\'' {
            // String literal: read until matching quote (no escapes).
            let quote = c;
            i += 1;
            let content_start = i;
            while i < len && bytes[i] != quote {
                i += 1;
            }
            // content_end is the index of the closing quote (or end of input).
            let content_end = i;
            // Consume the closing quote if present.
            if i < len {
                i += 1;
            }
            // Safety: trimmed is a valid &str and we sliced on byte boundaries
            // guaranteed by ASCII quote chars. The slice between two ASCII
            // quotes is itself valid UTF-8 because we never sliced inside a
            // multi-byte sequence.
            let content = std::str::from_utf8(&bytes[content_start..content_end])
                .unwrap_or("")
                .to_string();
            args.push(PlanArg::String(content));
        } else if c == b'-' || c.is_ascii_digit() {
            // Int literal: read [-]?\d+.
            let int_start = i;
            if c == b'-' {
                i += 1;
            }
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let int_end = i;
            let int_str = std::str::from_utf8(&bytes[int_start..int_end]).unwrap_or("");
            match int_str.parse::<i64>() {
                Ok(n) => args.push(PlanArg::Int(n)),
                Err(_) => args.push(PlanArg::Other(int_str.to_string())),
            }
        } else {
            // Anything else: consume until next comma at the top level
            // and store as Other.
            let other_start = i;
            while i < len && bytes[i] != b',' {
                i += 1;
            }
            let other_end = i;
            let other_str = std::str::from_utf8(&bytes[other_start..other_end])
                .unwrap_or("")
                .trim();
            if !other_str.is_empty() {
                args.push(PlanArg::Other(other_str.to_string()));
            }
        }
    }

    args
}

// ---------------------------------------------------------------------------
// Tests — focused unit tests + golden fixture coverage. The full 22-case
// fixture lives at `daemon/tests/parser_golden.rs` (integration test) so it
// can load the eval JSONL directly.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_args_string ----

    #[test]
    fn args_empty() {
        assert_eq!(parse_args_string(""), vec![]);
        assert_eq!(parse_args_string("  "), vec![]);
    }

    #[test]
    fn args_single_string() {
        assert_eq!(
            parse_args_string("\"wolves\""),
            vec![PlanArg::String("wolves".to_string())]
        );
    }

    #[test]
    fn args_string_and_int() {
        assert_eq!(
            parse_args_string("\"wolves\", 10"),
            vec![
                PlanArg::String("wolves".to_string()),
                PlanArg::Int(10),
            ]
        );
    }

    #[test]
    fn args_two_strings_with_reference() {
        assert_eq!(
            parse_args_string("\"$1\", \"https://example.org\""),
            vec![
                PlanArg::String("$1".to_string()),
                PlanArg::String("https://example.org".to_string()),
            ]
        );
    }

    #[test]
    fn args_empty_string_literal() {
        assert_eq!(
            parse_args_string("\"weaving datapods\", \"\""),
            vec![
                PlanArg::String("weaving datapods".to_string()),
                PlanArg::String(String::new()),
            ]
        );
    }

    #[test]
    fn args_string_with_spaces_and_url() {
        assert_eq!(
            parse_args_string("\"https://bair.berkeley.edu/blog/2024/05/29/tiny-agent\""),
            vec![PlanArg::String(
                "https://bair.berkeley.edu/blog/2024/05/29/tiny-agent".to_string()
            )]
        );
    }

    #[test]
    fn args_negative_int() {
        assert_eq!(parse_args_string("-3"), vec![PlanArg::Int(-3)]);
    }

    #[test]
    fn args_unknown_token_falls_back_to_other() {
        // Bare $1 (unquoted) is something the model doesn't emit but
        // the parser should still handle gracefully.
        assert_eq!(
            parse_args_string("$1"),
            vec![PlanArg::Other("$1".to_string())]
        );
    }

    // ---- parse_plan ----

    #[test]
    fn plan_single_search_then_join() {
        let raw = "1. search_local_index(\"wolves\", 10)\n\
                   Thought: I have searched the local index.\n\
                   2. join()";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].idx, 1);
        assert_eq!(steps[0].name, "search_local_index");
        assert_eq!(
            steps[0].args,
            vec![PlanArg::String("wolves".to_string()), PlanArg::Int(10)]
        );
        assert!(steps[0].references.is_empty());
        assert!(!steps[0].is_join());
        assert_eq!(steps[1].idx, 2);
        assert_eq!(steps[1].name, "join");
        assert!(steps[1].is_join());
    }

    #[test]
    fn plan_chain_with_dollar_reference() {
        let raw = "1. fetch_page(\"https://example.org/page\")\n\
                   2. scan_security(\"$1\", \"https://example.org/page\")\n\
                   3. join()";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1].name, "scan_security");
        assert_eq!(steps[1].references, vec![1]);
        assert_eq!(
            steps[1].args,
            vec![
                PlanArg::String("$1".to_string()),
                PlanArg::String("https://example.org/page".to_string()),
            ]
        );
    }

    #[test]
    fn plan_abstention_only_join() {
        let raw = "Thought: There is no tool available for sending email, so I cannot complete this request.\n\
                   1. join()";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].is_join());
        assert!(steps[0].args.is_empty());
    }

    #[test]
    fn plan_handles_continuation_token_lead_in() {
        // The model sometimes emits ". 1. tool(...)" with a leading dot
        // and space (chat-completion continuation token quirk). The
        // ACTION_RE's `(?:^|[^\w\$])` accepts this.
        let raw = ". 1. search_local_index(\"wolves\", 10)\n\
                   2. fetch_page(\"$1\")\n\
                   3. join()";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 3, "lead-in dot should not block parsing");
        assert_eq!(steps[0].name, "search_local_index");
        assert_eq!(steps[1].references, vec![1]);
    }

    #[test]
    fn plan_stops_at_first_join() {
        // Lines after a join should be ignored even if they look like
        // valid steps (the model sometimes hallucinates extra steps).
        let raw = "1. fetch_page(\"https://example.org\")\n\
                   2. join()\n\
                   3. extract_content(\"$1\", \"summary\")";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].name, "join");
    }

    #[test]
    fn plan_with_end_of_plan_sentinel() {
        let raw = "1. search_local_index(\"wolves\", 10)\n\
                   2. join()<END_OF_PLAN>";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps[1].is_join());
    }

    #[test]
    fn plan_three_step_chain() {
        let raw = "1. fetch_page(\"Lupus GitHub README\")\n\
                   2. extract_content(\"$1\", \"summary\")\n\
                   3. scan_security(\"$1\", \"Lupus GitHub README\")\n\
                   4. join()";
        let steps = parse_plan(raw).unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].name, "fetch_page");
        assert_eq!(steps[1].name, "extract_content");
        assert_eq!(steps[1].references, vec![1]);
        assert_eq!(steps[2].name, "scan_security");
        assert_eq!(steps[2].references, vec![1]);
        assert!(steps[3].is_join());
    }

    #[test]
    fn plan_empty_input() {
        assert_eq!(parse_plan("").unwrap(), vec![]);
    }

    #[test]
    fn plan_text_with_no_actions() {
        let raw = "Thought: I am thinking but not acting.\nNothing here.";
        assert_eq!(parse_plan(raw).unwrap(), vec![]);
    }
}
