//! TinyAgent‑based search agent with LoRA adapter hot‑swapping.
//!
//! Manages the search model lifecycle: load base weights, attach a LoRA
//! adapter (search or content), run the agent loop with tool calling.

pub mod prompt;
pub mod inference;
pub mod plan;
pub mod executor;
pub mod joinner;

use std::sync::{Arc, Mutex};

use crate::config::ModelsConfig;
use crate::error::LupusError;
use crate::protocol::{
    ComponentState, PlanStepRecord, SearchParams, SearchResponse, SearchResult,
    SummarizeParams, SummarizeResponse,
};

use self::executor::ExecutionRecord;
use self::inference::{InferenceEngine, MAX_PLANNER_TOKENS};

/// Known LoRA adapters that can be hot‑swapped onto the base model.
pub const ADAPTER_SEARCH: &str = "search";
pub const ADAPTER_CONTENT: &str = "content";

// FUNC_CALL_START / FUNC_CALL_END deleted in Phase 5. The pre-eval
// daemon scaffold assumed TinyAgent emitted JSON-wrapped tool calls
// inside `<|function_call|>...<|end_function_call|>` markers; Phase 1
// of the eval (docs/TINYAGENT_PHASE1_FINDINGS.md) confirmed empirically
// that TinyAgent emits LLMCompiler-format numbered plans instead. The
// new parser is at `daemon/src/agent/plan.rs`.

pub struct Agent {
    model_path: std::path::PathBuf,
    search_adapter_path: std::path::PathBuf,
    content_adapter_path: std::path::PathBuf,
    state: AgentState,
}

enum AgentState {
    Unloaded,
    Loaded {
        adapter: String,
        /// The single InferenceEngine instance, shared across calls via
        /// `std::sync::Mutex` (we use std rather than tokio because the
        /// lock is held inside `tokio::task::spawn_blocking` closures
        /// where async-aware locks aren't needed and would refuse to
        /// hold across await points). v1 serializes inference through
        /// this mutex; concurrent requests queue. Wrapped in Arc so the
        /// closures `spawn_blocking` runs can take ownership of a clone
        /// without moving the engine out of `self`.
        engine: Arc<Mutex<InferenceEngine>>,
    },
}

impl Agent {
    pub fn new(models: &ModelsConfig) -> Self {
        Self {
            model_path: models.search_base.clone(),
            search_adapter_path: models.search_adapter.clone(),
            content_adapter_path: models.content_adapter.clone(),
            state: AgentState::Unloaded,
        }
    }

    /// Load the base GGUF + search LoRA adapter into a fresh
    /// `InferenceEngine`. After this returns, the agent is ready to
    /// serve `search()` requests. Synchronous CPU work (model mmap +
    /// LoRA file read) is wrapped in `spawn_blocking` so we don't
    /// stall the tokio reactor.
    pub async fn load(&mut self) -> Result<(), LupusError> {
        tracing::info!(
            "Loading search base model from {}",
            self.model_path.display()
        );
        tracing::info!(
            "Loading search LoRA adapter from {}",
            self.search_adapter_path.display()
        );

        let model_path = self.model_path.clone();
        let lora_path = self.search_adapter_path.clone();

        let engine = tokio::task::spawn_blocking(move || {
            InferenceEngine::load(&model_path, &lora_path)
        })
        .await
        .map_err(|e| {
            LupusError::ModelLoadFailed(format!("spawn_blocking join: {e}"))
        })??;

        self.state = AgentState::Loaded {
            adapter: ADAPTER_SEARCH.into(),
            engine: Arc::new(Mutex::new(engine)),
        };
        tracing::info!("Search model loaded with {} adapter", ADAPTER_SEARCH);
        Ok(())
    }

    /// Hot‑swap the LoRA adapter (~100ms target).
    ///
    /// v1 only supports the search adapter — the content adapter is a
    /// separate trained LoRA we haven't built yet. Calls for `content`
    /// return `AdapterNotFound` until that work lands.
    pub async fn swap_adapter(&mut self, adapter: &str) -> Result<(), LupusError> {
        let _adapter_path = match adapter {
            ADAPTER_SEARCH => &self.search_adapter_path,
            ADAPTER_CONTENT => &self.content_adapter_path,
            other => return Err(LupusError::AdapterNotFound(other.into())),
        };

        tracing::info!("swap_adapter requested: {adapter} (v1 stub — re-using current engine)");

        // v1 stub: the InferenceEngine attaches the LoRA per-context
        // via context.lora_adapter_set, so true hot-swap means loading
        // a new LoRA and switching which one infer_blocking attaches.
        // For now we just record the requested name; the actual
        // multi-adapter wiring lands when the content adapter exists.
        if let AgentState::Loaded { adapter: name, .. } = &mut self.state {
            *name = adapter.to_string();
        }
        Ok(())
    }

    /// Run a hunt — process a search query through the full LLMCompiler
    /// agent loop. The "hunt" is what the agent does when given a query;
    /// the IPC method is still named `search` because that's the user's
    /// verb (the user asks for a search; under the hood the wolf hunts).
    ///
    /// Steps:
    /// 1. Build the cached planner system prompt
    ///    ([`prompt::planner_system_prompt`])
    /// 2. Run the planner inference call WITH the trained LoRA attached
    ///    (greedy, stop on `<END_OF_PLAN>`)
    /// 3. Parse the raw output into a `Vec<PlanStep>`
    ///    ([`plan::parse_plan`])
    /// 4. Execute the plan sequentially against the tool dispatcher,
    ///    resolving `$N` references against prior observations
    ///    ([`executor::execute_plan`])
    /// 5. Run the joinner second pass WITHOUT the LoRA to synthesize
    ///    a natural-language `Action: Finish(<answer>)` reply
    ///    ([`joinner::run_joinner`])
    /// 6. Return the joinner answer + executed plan in `SearchResponse`
    ///
    /// All inference is wrapped in `tokio::task::spawn_blocking` because
    /// llama.cpp inference is multi-second CPU work that must not
    /// block the tokio reactor.
    pub async fn hunt(&self, params: SearchParams) -> Result<SearchResponse, LupusError> {
        self.require_loaded()?;

        tracing::debug!(
            "Hunt query: {:?} scope: {:?}",
            params.query,
            params.scope
        );

        let engine_arc = match &self.state {
            AgentState::Loaded { engine, .. } => Arc::clone(engine),
            AgentState::Unloaded => {
                return Err(LupusError::ModelNotLoaded("search".into()));
            }
        };

        let user_prompt = format!("Question: {}", params.query);

        // Phase 1: planner inference (with LoRA).
        let planner_engine = Arc::clone(&engine_arc);
        let planner_user = user_prompt.clone();
        let raw_plan = tokio::task::spawn_blocking(move || -> Result<String, LupusError> {
            let mut engine = planner_engine
                .lock()
                .map_err(|e| LupusError::Inference(format!("planner mutex poisoned: {e}")))?;
            engine.infer_blocking(
                prompt::planner_system_prompt(),
                &planner_user,
                MAX_PLANNER_TOKENS,
                /* use_lora */ true,
                inference::PLANNER_STOP_STRINGS,
            )
        })
        .await
        .map_err(|e| LupusError::Inference(format!("spawn_blocking join (planner): {e}")))??;

        tracing::debug!(?raw_plan, "planner output");

        // Phase 2: parse the plan.
        let plan_steps = plan::parse_plan(&raw_plan)?;
        tracing::debug!(steps = plan_steps.len(), "parsed plan");

        // Phase 3: execute the plan against the tool dispatcher.
        let records = executor::execute_plan(&plan_steps).await;

        // Phase 4: joinner second pass (without LoRA).
        let joinner_engine = Arc::clone(&engine_arc);
        let joinner_user = params.query.clone();
        let joinner_records = records.clone();
        let joinner_out = tokio::task::spawn_blocking(move || {
            let mut engine = joinner_engine
                .lock()
                .map_err(|e| LupusError::Inference(format!("joinner mutex poisoned: {e}")))?;
            joinner::run_joinner(&mut engine, &joinner_user, &joinner_records)
        })
        .await
        .map_err(|e| LupusError::Inference(format!("spawn_blocking join (joinner): {e}")))??;

        tracing::info!(
            joinner_raw = %joinner_out.raw_output,
            joinner_thought = %joinner_out.thought,
            joinner_answer = %joinner_out.answer,
            joinner_is_replan = joinner_out.is_replan,
            "joinner output"
        );

        if joinner_out.is_replan {
            // v1 doesn't loop on Replan (decision D in the integration plan).
            // Surface as a clean error so the joinner output isn't lost.
            return Err(LupusError::Inference(format!(
                "joinner requested Replan; v1 does not support replanning. \
                 thought: {:?}",
                joinner_out.thought
            )));
        }

        // Build the wire-format response.
        let plan_records: Vec<PlanStepRecord> = records.iter().map(record_to_wire).collect();
        let results = harvest_search_results(&records);

        Ok(SearchResponse {
            text_answer: if joinner_out.answer.is_empty() {
                None
            } else {
                Some(joinner_out.answer)
            },
            plan: Some(plan_records),
            results,
        })
    }

    /// Summarize page content using the content adapter.
    pub async fn summarize(&self, params: SummarizeParams) -> Result<SummarizeResponse, LupusError> {
        self.require_loaded()?;

        let _html = params.html.or(params.url).unwrap_or_default();

        // TODO: Ensure content adapter is loaded, run summarization
        //   if current_adapter != ADAPTER_CONTENT { swap_adapter(ADAPTER_CONTENT)?; }
        //   let output = model.generate(&summarize_prompt(html), max_tokens)?;

        Ok(SummarizeResponse {
            title: String::new(),
            summary: String::new(),
        })
    }

    pub fn current_adapter(&self) -> &str {
        match &self.state {
            AgentState::Loaded { adapter, .. } => adapter,
            AgentState::Unloaded => "none",
        }
    }

    pub fn component_state(&self) -> ComponentState {
        match &self.state {
            AgentState::Loaded { .. } => ComponentState::Ready,
            AgentState::Unloaded => ComponentState::Loading,
        }
    }

    fn require_loaded(&self) -> Result<(), LupusError> {
        match &self.state {
            AgentState::Loaded { .. } => Ok(()),
            AgentState::Unloaded => Err(LupusError::ModelNotLoaded("search".into())),
        }
    }
}

// ---------------------------------------------------------------------------
// Wire-format conversion helpers
// ---------------------------------------------------------------------------

/// Convert an internal [`ExecutionRecord`] to the public `PlanStepRecord`
/// type the WebSocket clients see in `SearchResponse.plan`. Strips the
/// internal `PlanStep` typing in favor of the simpler raw_args string.
fn record_to_wire(record: &ExecutionRecord) -> PlanStepRecord {
    PlanStepRecord {
        idx: record.step.idx,
        tool: record.step.name.clone(),
        raw_args: record.step.raw_args.clone(),
        observation: record.observation.clone(),
        error: record.error.clone(),
        is_join: record.step.is_join(),
    }
}

/// Walk the executed plan and collect any structured search hits into
/// the legacy `SearchResult` shape so the browser UI's old "search
/// results" rendering still works for plans that include a search step.
///
/// Pulls from `search_local_index` (`{results: [{url, title, summary,
/// score}]}`) and `search_subnet` (`{matches: [{title, url, description,
/// commitment}]}`) observations. Other tool observations don't map to
/// the SearchResult shape and are silently skipped.
///
/// This is best-effort: returns an empty Vec if no search step ran. The
/// browser should treat the absence of `results` as "see text_answer
/// for the reply" rather than "no matches".
fn harvest_search_results(records: &[ExecutionRecord]) -> Vec<SearchResult> {
    let mut out = Vec::new();
    for record in records {
        let Some(obs) = &record.observation else {
            continue;
        };
        match record.step.name.as_str() {
            "search_local_index" => {
                if let Some(arr) = obs.get("results").and_then(|v| v.as_array()) {
                    for item in arr {
                        let title = item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let url = item
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let summary = item
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let trust_score = item
                            .get("trust_score")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u8;
                        let score = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        out.push(SearchResult {
                            title,
                            url,
                            summary,
                            trust_score,
                            commitment: score,
                        });
                    }
                }
            }
            "search_subnet" => {
                if let Some(arr) = obs.get("matches").and_then(|v| v.as_array()) {
                    for item in arr {
                        let title = item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let url = item
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let summary = item
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let commitment = item
                            .get("commitment")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        out.push(SearchResult {
                            title,
                            url,
                            summary,
                            trust_score: 0,
                            commitment,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}
