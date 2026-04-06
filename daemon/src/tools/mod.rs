//! Tool registry — functions the TinyAgent model can call during the
//! agent loop. Each tool has a schema (for the system prompt) and an
//! execute function.

pub mod search_subnet;
pub mod search_local;
pub mod fetch_page;
pub mod extract_content;
pub mod scan_security;
pub mod crawl_index;

use serde::{Deserialize, Serialize};

use crate::error::LupusError;

/// Schema description for a single tool, included in the agent system prompt
/// so the model knows what functions it can call.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

/// A parsed tool call from model output.
#[derive(Debug, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// All registered tool schemas.
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

/// Build the system prompt section describing available tools.
pub fn system_prompt() -> String {
    let mut prompt = String::from("You have access to the following tools:\n\n");
    for schema in schemas() {
        prompt.push_str(&format!(
            "### {}\n{}\nParameters: {}\n\n",
            schema.name,
            schema.description,
            serde_json::to_string_pretty(&schema.parameters).unwrap_or_default(),
        ));
    }
    prompt.push_str(
        "To call a tool, output:\n\
         <|function_call|> {\"name\": \"tool_name\", \"arguments\": {...}} <|end_function_call|>\n\
         Wait for the result before continuing.\n"
    );
    prompt
}

/// Dispatch a tool call by name. Returns the tool's JSON output.
pub async fn execute(call: &ToolCall) -> Result<serde_json::Value, LupusError> {
    match call.name.as_str() {
        "search_subnet" => search_subnet::execute(call.arguments.clone()).await,
        "search_local_index" => search_local::execute(call.arguments.clone()).await,
        "fetch_page" => fetch_page::execute(call.arguments.clone()).await,
        "extract_content" => extract_content::execute(call.arguments.clone()).await,
        "scan_security" => scan_security::execute(call.arguments.clone()).await,
        "crawl_index" => crawl_index::execute(call.arguments.clone()).await,
        other => Err(LupusError::ToolError {
            tool: other.into(),
            message: "unknown tool".into(),
        }),
    }
}

/// Parse tool calls from raw model output. Returns all calls found.
pub fn parse_tool_calls(output: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut remaining = output;

    while let Some(start) = remaining.find(crate::agent::FUNC_CALL_START) {
        let after_marker = start + crate::agent::FUNC_CALL_START.len();
        if let Some(end) = remaining[after_marker..].find(crate::agent::FUNC_CALL_END) {
            let json_str = remaining[after_marker..after_marker + end].trim();
            if let Ok(call) = serde_json::from_str::<ToolCall>(json_str) {
                calls.push(call);
            }
            remaining = &remaining[after_marker + end + crate::agent::FUNC_CALL_END.len()..];
        } else {
            break;
        }
    }

    calls
}
