//! Plan executor — runs the parsed `PlanStep`s from
//! [`crate::agent::plan::parse_plan`] against the tool dispatcher.
//!
//! Sequential execution per decision E in the daemon integration plan.
//! Parallel topo-sort execution would let independent steps run
//! concurrently but is deferred to v2; chains in our 22-case eval are
//! all ≤3 steps so the sequential cost is bounded.
//!
//! ## `$N` reference resolution (decision H — smart per-tool extraction)
//!
//! When a step's arg contains a `$N` reference (e.g.
//! `fetch_page("$1")`), the executor needs to substitute the
//! observation from step N into the arg. Upstream BAIR's executor does
//! a dumb `str(observation)` substitution; that works for their
//! Apple-app tools where outputs are primitive strings but fails for
//! Lupus's tools which return structured JSON. Without smart extraction,
//! `fetch_page` would receive a stringified `{"results": [...]}` JSON
//! blob as its URL argument and fail.
//!
//! [`smart_extract`] hardcodes the (producer_tool, consuming_arg_field)
//! → JSON path table for our 6-tool surface. Adding a new tool means
//! adding a new arm. The table is small, finite, and stable.
//!
//! Limitation: always picks the first result of any list-shaped
//! observation (`.results[0]`, `.matches[0]`). The "second result" intent
//! is silently dropped if the user expresses it. Documented as the v1
//! caveat in `docs/TINYAGENT_PHASE3_PLAN.md` decision H.
//!
//! ## Error handling
//!
//! Per-step errors are stored in [`ExecutionRecord::error`] and
//! execution continues. The joinner sees the failure as part of the
//! observation list and decides whether to surface it to the user or
//! recover (Replan, deferred to v2).

use serde_json::{json, Value};

use crate::agent::plan::{PlanArg, PlanStep};
use crate::error::LupusError;
use crate::tools;

/// One step's execution result. Mirrors the upstream BAIR `Task` struct's
/// observation field but adds an explicit `error` channel for failures.
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    /// The plan step that was executed (or skipped, for `join`).
    pub step: PlanStep,
    /// The tool's JSON output, if execution succeeded. `None` for
    /// `join()` steps and for steps that errored.
    pub observation: Option<Value>,
    /// Human-readable error if the step failed during arg coercion or
    /// tool dispatch. Mutually exclusive with `observation`.
    pub error: Option<String>,
}

impl ExecutionRecord {
    /// True if this record represents a `join`/`join_finish`/`join_replan`
    /// terminator that was added to the record list but not actually
    /// dispatched as a tool.
    pub fn is_join(&self) -> bool {
        self.step.is_join()
    }

    /// True if this record holds a successful tool observation.
    pub fn is_success(&self) -> bool {
        self.observation.is_some() && self.error.is_none()
    }
}

/// Execute a parsed plan sequentially. Returns one [`ExecutionRecord`]
/// per step in the plan, in the same order. Errors during arg coercion
/// or tool dispatch are recorded but do not stop execution — the
/// joinner sees the full record list and decides what to surface.
///
/// `join` steps are pushed as-is with `observation: None, error: None`
/// — they're plan terminators, not tool calls.
pub async fn execute_plan(plan: &[PlanStep]) -> Vec<ExecutionRecord> {
    let mut records: Vec<ExecutionRecord> = Vec::with_capacity(plan.len());

    for step in plan {
        if step.is_join() {
            records.push(ExecutionRecord {
                step: step.clone(),
                observation: None,
                error: None,
            });
            continue;
        }

        // Coerce positional args (with `$N` substitution) into the
        // named JSON shape the tool's `execute` expects.
        let coerced = match coerce_args(&step.name, &step.args, &records) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(step.idx, step.name, error = %e, "arg coercion failed");
                records.push(ExecutionRecord {
                    step: step.clone(),
                    observation: None,
                    error: Some(format!("arg coercion: {e}")),
                });
                continue;
            }
        };

        // Dispatch through the tool registry. The dispatcher's hard
        // validation in `daemon/src/tools/mod.rs::execute` rejects
        // hallucinated tool names — that's the unconditional safety net.
        match tools::execute(&step.name, coerced).await {
            Ok(observation) => records.push(ExecutionRecord {
                step: step.clone(),
                observation: Some(observation),
                error: None,
            }),
            Err(e) => {
                tracing::warn!(step.idx, step.name, error = %e, "tool dispatch failed");
                records.push(ExecutionRecord {
                    step: step.clone(),
                    observation: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    records
}

// ---------------------------------------------------------------------------
// Per-tool positional → named arg coercion
// ---------------------------------------------------------------------------

/// Convert a step's positional `Vec<PlanArg>` into the named JSON object
/// each tool's `execute` function deserializes via serde.
///
/// This is the spot where the LLMCompiler positional convention
/// (`tool("query", 10)`) meets the tool implementation's named-param
/// convention (`{"query": "...", "top_k": 10}`). The mapping is hardcoded
/// per tool — adding a new Lupus tool means adding a new match arm here
/// AND in [`tools::execute`].
///
/// `$N` references inside string args are resolved against `prior` via
/// [`smart_extract`].
fn coerce_args(
    name: &str,
    args: &[PlanArg],
    prior: &[ExecutionRecord],
) -> Result<Value, LupusError> {
    match name {
        "search_subnet" => {
            let query = string_arg(args, 0, name, "query", prior)?;
            let scope = string_arg(args, 1, name, "scope", prior).unwrap_or_default();
            Ok(json!({ "query": query, "scope": scope }))
        }
        "search_local_index" => {
            let query = string_arg(args, 0, name, "query", prior)?;
            let top_k = int_arg(args, 1).unwrap_or(10);
            Ok(json!({ "query": query, "top_k": top_k }))
        }
        "fetch_page" => {
            let url = string_arg(args, 0, name, "url", prior)?;
            Ok(json!({ "url": url }))
        }
        "extract_content" => {
            let html = string_arg(args, 0, "extract_content", "html", prior)?;
            let format = string_arg(args, 1, "extract_content", "format", prior)
                .unwrap_or_else(|_| "full".to_string());
            Ok(json!({ "html": html, "format": format }))
        }
        "scan_security" => {
            let html = string_arg(args, 0, "scan_security", "html", prior)?;
            let url = string_arg(args, 1, "scan_security", "url", prior)?;
            Ok(json!({ "html": html, "url": url }))
        }
        "crawl_index" => {
            let source = string_arg(args, 0, "crawl_index", "source", prior)?;
            Ok(json!({ "source": source }))
        }
        other => Err(LupusError::ToolError {
            tool: other.into(),
            message: format!("no arg coercion mapping for tool {other:?}"),
        }),
    }
}

/// Pull the i-th positional arg as a string, resolving `$N` references
/// to prior step observations via the smart extraction table.
///
/// Returns `Err(LupusError::Inference)` if the arg slot is missing.
fn string_arg(
    args: &[PlanArg],
    i: usize,
    tool: &str,
    field: &str,
    prior: &[ExecutionRecord],
) -> Result<String, LupusError> {
    let arg = args.get(i).ok_or_else(|| {
        LupusError::Inference(format!(
            "{tool}: missing positional arg #{i} ({field})"
        ))
    })?;

    match arg {
        PlanArg::String(s) => {
            // Whole-string $N reference (e.g. "$1") → smart extract.
            // We deliberately don't try to interpolate $N inside longer
            // strings since none of our 22 eval cases need it.
            if let Some(n) = parse_dollar_ref(s) {
                smart_extract(tool, field, n, prior)
            } else {
                Ok(s.clone())
            }
        }
        PlanArg::Int(i) => Ok(i.to_string()),
        PlanArg::Other(s) => Ok(s.clone()),
    }
}

/// Pull the i-th positional arg as an integer. Returns `None` if the
/// slot is missing or the arg is the wrong type — the caller decides on
/// a default value.
fn int_arg(args: &[PlanArg], i: usize) -> Option<i64> {
    match args.get(i)? {
        PlanArg::Int(n) => Some(*n),
        PlanArg::String(s) => s.parse().ok(),
        PlanArg::Other(_) => None,
    }
}

/// Parse a single `$N` or `${N}` reference. Returns `Some(N)` only if
/// the entire string is a reference; partial matches inside longer
/// strings return `None`. Invalid references (`$abc`, `$`, etc.) also
/// return `None`.
fn parse_dollar_ref(s: &str) -> Option<u32> {
    let inner = s.strip_prefix('$')?;
    let digits = if let Some(rest) = inner.strip_prefix('{') {
        rest.strip_suffix('}')?
    } else {
        inner
    };
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

// ---------------------------------------------------------------------------
// Smart $N extraction table — decision H from the integration plan.
// ---------------------------------------------------------------------------

/// Resolve a `$N` reference by looking up step N in `prior` and
/// extracting the right field based on the producer tool and the
/// consuming tool's arg slot.
///
/// This is the "smart" half of decision H. Without it, dumb stringify
/// of the producer's full JSON observation would feed garbage into the
/// consumer (e.g. `fetch_page("{\"results\":[...]}")`) and break every
/// multi-step chain.
///
/// The (producer, consumer_field) pairs that actually appear in our
/// 22-case eval are hardcoded as match arms. New tools need new arms.
///
/// Always picks the first list element when the producer returns a
/// list-shaped observation. The "second result" intent is silently
/// dropped — see decision H caveats in
/// `docs/TINYAGENT_PHASE3_PLAN.md`.
fn smart_extract(
    consumer_tool: &str,
    consumer_field: &str,
    n: u32,
    prior: &[ExecutionRecord],
) -> Result<String, LupusError> {
    let producer = prior
        .iter()
        .find(|r| r.step.idx == n)
        .ok_or_else(|| {
            LupusError::Inference(format!(
                "{consumer_tool}.{consumer_field}: $\\{n} references step \
                 that doesn't exist in the executed plan"
            ))
        })?;

    let obs = producer.observation.as_ref().ok_or_else(|| {
        LupusError::Inference(format!(
            "{consumer_tool}.{consumer_field}: $\\{n} references step \
             that errored or was a join terminator"
        ))
    })?;

    let producer_name = producer.step.name.as_str();

    match (producer_name, consumer_field) {
        // Search results → pluck the first hit's URL for any URL slot.
        ("search_local_index", "url" | "source") => obs
            .pointer("/results/0/url")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| {
                LupusError::Inference(
                    "search_local_index observation has no .results[0].url".into(),
                )
            }),
        ("search_subnet", "url" | "source") => obs
            .pointer("/matches/0/url")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| {
                LupusError::Inference(
                    "search_subnet observation has no .matches[0].url".into(),
                )
            }),

        // fetch_page produces { url, content_type, body, status }. Different
        // consumers want different fields:
        // - extract_content.html and scan_security.html want .body
        // - scan_security.url wants .url
        ("fetch_page", "html") => obs
            .get("body")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| LupusError::Inference("fetch_page observation has no .body".into())),
        ("fetch_page", "url") => obs
            .get("url")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| LupusError::Inference("fetch_page observation has no .url".into())),

        // crawl_index produces { indexed, url, title }. The url field is the
        // canonical handle if anything downstream references the indexed page.
        ("crawl_index", "url" | "source") => obs
            .get("url")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| LupusError::Inference("crawl_index observation has no .url".into())),

        // extract_content produces { title, summary, content, keywords,
        // content_type }. If a downstream tool wanted to chain off the
        // summary text, .summary is the natural pick. None of our 22-case
        // eval queries actually do this, so this arm is speculative; the
        // fallback below catches anything else.
        ("extract_content", _) => obs
            .get("summary")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| {
                LupusError::Inference("extract_content observation has no .summary".into())
            }),

        // scan_security observations rarely chain into anything useful.
        // Fall through to the dumb stringify so the joinner at least sees
        // *something* — the consumer will likely error out, which is
        // visible in the records.
        _ => {
            tracing::warn!(
                producer = producer_name,
                consumer_tool,
                consumer_field,
                "no smart extraction rule for ($N producer, consumer field) pair; \
                 falling back to JSON stringify"
            );
            Ok(obs.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — focus on logic that doesn't need real tool dispatch (the tools
// are still stubs and return empty results). The hard parts to test are
// the arg coercion table and the smart $N extraction; the actual
// execute_plan glue is exercised end-to-end via the eval_smoke binary in
// Phase 7.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn step(idx: u32, name: &str, args: Vec<PlanArg>) -> PlanStep {
        let raw_args = args
            .iter()
            .map(|a| match a {
                PlanArg::String(s) => format!("\"{s}\""),
                PlanArg::Int(i) => i.to_string(),
                PlanArg::Other(s) => s.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let references = args
            .iter()
            .filter_map(|a| match a {
                PlanArg::String(s) => parse_dollar_ref(s),
                _ => None,
            })
            .collect();
        PlanStep {
            idx,
            name: name.to_string(),
            raw_args,
            args,
            references,
        }
    }

    fn record(step: PlanStep, observation: Value) -> ExecutionRecord {
        ExecutionRecord {
            step,
            observation: Some(observation),
            error: None,
        }
    }

    // ---- parse_dollar_ref ----

    #[test]
    fn dollar_ref_plain() {
        assert_eq!(parse_dollar_ref("$1"), Some(1));
        assert_eq!(parse_dollar_ref("$10"), Some(10));
    }

    #[test]
    fn dollar_ref_braced() {
        assert_eq!(parse_dollar_ref("${1}"), Some(1));
        assert_eq!(parse_dollar_ref("${42}"), Some(42));
    }

    #[test]
    fn dollar_ref_rejects_non_reference() {
        assert_eq!(parse_dollar_ref(""), None);
        assert_eq!(parse_dollar_ref("$"), None);
        assert_eq!(parse_dollar_ref("$abc"), None);
        assert_eq!(parse_dollar_ref("hello"), None);
        assert_eq!(parse_dollar_ref("$1foo"), None);
        assert_eq!(parse_dollar_ref("foo$1"), None);
    }

    // ---- int_arg ----

    #[test]
    fn int_arg_picks_int() {
        assert_eq!(
            int_arg(&[PlanArg::String("foo".into()), PlanArg::Int(7)], 1),
            Some(7)
        );
    }

    #[test]
    fn int_arg_parses_string_int() {
        assert_eq!(
            int_arg(&[PlanArg::String("42".into())], 0),
            Some(42)
        );
    }

    #[test]
    fn int_arg_returns_none_on_missing() {
        assert_eq!(int_arg(&[], 0), None);
    }

    // ---- coerce_args (no $N references) ----

    #[test]
    fn coerce_search_local_index_two_args() {
        let args = vec![PlanArg::String("wolves".into()), PlanArg::Int(10)];
        let result = coerce_args("search_local_index", &args, &[]).unwrap();
        assert_eq!(result, json!({"query": "wolves", "top_k": 10}));
    }

    #[test]
    fn coerce_search_local_index_default_top_k() {
        let args = vec![PlanArg::String("wolves".into())];
        let result = coerce_args("search_local_index", &args, &[]).unwrap();
        assert_eq!(result, json!({"query": "wolves", "top_k": 10}));
    }

    #[test]
    fn coerce_fetch_page_url_only() {
        let args = vec![PlanArg::String("https://example.org".into())];
        let result = coerce_args("fetch_page", &args, &[]).unwrap();
        assert_eq!(result, json!({"url": "https://example.org"}));
    }

    #[test]
    fn coerce_extract_content_two_args() {
        let args = vec![
            PlanArg::String("<html>...</html>".into()),
            PlanArg::String("summary".into()),
        ];
        let result = coerce_args("extract_content", &args, &[]).unwrap();
        assert_eq!(
            result,
            json!({"html": "<html>...</html>", "format": "summary"})
        );
    }

    #[test]
    fn coerce_search_subnet_with_empty_scope() {
        let args = vec![
            PlanArg::String("weaving".into()),
            PlanArg::String("".into()),
        ];
        let result = coerce_args("search_subnet", &args, &[]).unwrap();
        assert_eq!(result, json!({"query": "weaving", "scope": ""}));
    }

    #[test]
    fn coerce_unknown_tool_errors() {
        let result = coerce_args("send_email", &[], &[]);
        assert!(result.is_err());
        match result {
            Err(LupusError::ToolError { tool, message }) => {
                assert_eq!(tool, "send_email");
                assert!(message.contains("no arg coercion mapping"));
            }
            _ => panic!("expected ToolError"),
        }
    }

    // ---- smart_extract (the heart of decision H) ----

    #[test]
    fn smart_extract_search_local_index_to_fetch_url() {
        let prior_step = step(1, "search_local_index", vec![PlanArg::String("wolves".into())]);
        let prior = vec![record(
            prior_step,
            json!({"results": [{"url": "https://wiki/wolves", "title": "Wolves"}]}),
        )];
        let result = smart_extract("fetch_page", "url", 1, &prior).unwrap();
        assert_eq!(result, "https://wiki/wolves");
    }

    #[test]
    fn smart_extract_search_subnet_to_crawl_source() {
        let prior_step = step(1, "search_subnet", vec![PlanArg::String("weaving".into())]);
        let prior = vec![record(
            prior_step,
            json!({"matches": [{"url": "hvym://datapods/weaving", "title": "Weaving"}]}),
        )];
        let result = smart_extract("crawl_index", "source", 1, &prior).unwrap();
        assert_eq!(result, "hvym://datapods/weaving");
    }

    #[test]
    fn smart_extract_fetch_page_body_to_extract_html() {
        let prior_step = step(1, "fetch_page", vec![PlanArg::String("https://x.org".into())]);
        let prior = vec![record(
            prior_step,
            json!({"url": "https://x.org", "body": "<html>hello</html>", "status": "ok"}),
        )];
        let result = smart_extract("extract_content", "html", 1, &prior).unwrap();
        assert_eq!(result, "<html>hello</html>");
    }

    #[test]
    fn smart_extract_fetch_page_url_to_scan_url() {
        let prior_step = step(1, "fetch_page", vec![PlanArg::String("https://x.org".into())]);
        let prior = vec![record(
            prior_step,
            json!({"url": "https://x.org", "body": "<html>...</html>"}),
        )];
        let result = smart_extract("scan_security", "url", 1, &prior).unwrap();
        assert_eq!(result, "https://x.org");
    }

    #[test]
    fn smart_extract_missing_step_errors() {
        let result = smart_extract("fetch_page", "url", 99, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn smart_extract_errored_step_errors() {
        let prior = vec![ExecutionRecord {
            step: step(1, "search_local_index", vec![]),
            observation: None,
            error: Some("simulated tool error".into()),
        }];
        let result = smart_extract("fetch_page", "url", 1, &prior);
        assert!(result.is_err());
    }

    #[test]
    fn smart_extract_search_local_no_results_errors() {
        let prior = vec![record(
            step(1, "search_local_index", vec![]),
            json!({"results": []}),
        )];
        let result = smart_extract("fetch_page", "url", 1, &prior);
        assert!(result.is_err());
    }

    // ---- coerce_args with $N reference ----

    #[test]
    fn coerce_fetch_with_dollar_ref_resolves_via_search() {
        let prior = vec![record(
            step(1, "search_local_index", vec![PlanArg::String("wolves".into())]),
            json!({"results": [{"url": "https://wiki/wolves"}]}),
        )];
        let args = vec![PlanArg::String("$1".into())];
        let result = coerce_args("fetch_page", &args, &prior).unwrap();
        assert_eq!(result, json!({"url": "https://wiki/wolves"}));
    }

    #[test]
    fn coerce_scan_with_dollar_ref_for_html_field() {
        let prior = vec![record(
            step(1, "fetch_page", vec![PlanArg::String("https://x.org".into())]),
            json!({"url": "https://x.org", "body": "<html>page</html>"}),
        )];
        let args = vec![
            PlanArg::String("$1".into()),
            PlanArg::String("https://x.org".into()),
        ];
        let result = coerce_args("scan_security", &args, &prior).unwrap();
        assert_eq!(
            result,
            json!({"html": "<html>page</html>", "url": "https://x.org"})
        );
    }
}
