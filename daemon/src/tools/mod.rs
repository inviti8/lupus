//! Tool registry — the 6 functions the planner LoRA can call during the
//! agent loop. Each tool has a schema (kept for OpenAPI export, even
//! though the planner prompt no longer uses it — see decision G in the
//! daemon integration plan) and an `execute` function.
//!
//! The dispatcher in [`execute`] is the **unconditional safety net**
//! against hallucinated tool calls: any name not in the match arm
//! returns `LupusError::ToolError { message: "unknown tool" }`. Even if
//! the planner LoRA emits a fabricated tool name like `compose_email`,
//! it cannot execute through this dispatcher. **Do not weaken this
//! guarantee.**
//!
//! The pre-Phase 5 version of this file declared `parse_tool_calls`,
//! `system_prompt`, and a `ToolCall` struct that assumed TinyAgent
//! emitted JSON-wrapped function call markers (`<|function_call|>...`).
//! That was the wrong format premise refuted in Phase 1 of the eval; it
//! was deleted as part of Phase 5. The replacement parser is at
//! `daemon/src/agent/plan.rs` and the replacement system prompt is at
//! `daemon/src/agent/prompt.rs`.

pub mod search_subnet;
pub mod search_local;
pub mod fetch_page;
pub mod extract_content;
pub mod scan_security;
pub mod crawl_index;

use serde::Serialize;

use crate::error::LupusError;

/// Schema description for a single tool. Kept for potential OpenAPI
/// export; not used for prompt rendering anymore (the Phase 2 prompt
/// port at `daemon/src/agent/prompt.rs` has its own constants).
#[derive(Debug, Clone, Serialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

/// All registered tool schemas. Currently only consumed by tests; the
/// production prompt builder hardcodes the descriptions in
/// `agent::prompt`.
pub fn schemas() -> Vec<ToolSchema> {
    vec![
        search_subnet::schema(),
        search_local::schema(),
        fetch_page::schema(),
        extract_content::schema(),
        scan_security::schema(),
        crawl_index::schema(),
    ]
}

/// Dispatch a tool call by name. Returns the tool's JSON output, or
/// `LupusError::ToolError { message: "unknown tool" }` for any name
/// that isn't in our 6-tool surface.
///
/// # Safety net
///
/// The `_ =>` arm is the daemon's hard floor against planner
/// hallucinations. The trained LoRA's tool selection is at 95.5% on the
/// 22-case eval but the failure mode for the remaining 4.5% is
/// emitting a hallucinated tool name (e.g. `compose_email`,
/// `send_email`). Without this validation, those calls would propagate
/// through the executor and either crash or silently misbehave. With
/// it, hallucinated names produce a clean error that the joinner can
/// surface to the user. **Keep this arm.**
pub async fn execute(
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, LupusError> {
    match name {
        "search_subnet" => search_subnet::execute(args).await,
        "search_local_index" => search_local::execute(args).await,
        "fetch_page" => fetch_page::execute(args).await,
        "extract_content" => extract_content::execute(args).await,
        "scan_security" => scan_security::execute(args).await,
        "crawl_index" => crawl_index::execute(args).await,
        other => Err(LupusError::ToolError {
            tool: other.into(),
            message: "unknown tool".into(),
        }),
    }
}
