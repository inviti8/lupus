//! TinyAgent‑based search agent with LoRA adapter hot‑swapping.
//!
//! Manages the search model lifecycle: load base weights, attach a LoRA
//! adapter (search or content), run the agent loop with tool calling.

pub mod prompt;
pub mod inference;
pub mod plan;
pub mod executor;

use crate::config::ModelsConfig;
use crate::error::LupusError;
use crate::protocol::{
    ComponentState, SearchParams, SearchResponse,
    SummarizeParams, SummarizeResponse,
};

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
        // TODO: llama_cpp model handle goes here
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

    /// Load the base model and attach the default (search) adapter.
    pub async fn load(&mut self) -> Result<(), LupusError> {
        tracing::info!("Loading search base model from {}", self.model_path.display());

        // TODO: Load GGUF via llama-cpp-2
        //   let model = LlamaModel::load_from_file(&self.model_path, params)?;
        //   model.apply_lora(&self.search_adapter_path)?;

        self.state = AgentState::Loaded {
            adapter: ADAPTER_SEARCH.into(),
        };
        tracing::info!("Search model loaded with {} adapter", ADAPTER_SEARCH);
        Ok(())
    }

    /// Hot‑swap the LoRA adapter (~100ms target).
    pub async fn swap_adapter(&mut self, adapter: &str) -> Result<(), LupusError> {
        let adapter_path = match adapter {
            ADAPTER_SEARCH => &self.search_adapter_path,
            ADAPTER_CONTENT => &self.content_adapter_path,
            other => return Err(LupusError::AdapterNotFound(other.into())),
        };

        tracing::info!("Swapping to {} adapter ({})", adapter, adapter_path.display());

        // TODO: Unload current adapter, load new one
        //   model.clear_lora()?;
        //   model.apply_lora(adapter_path)?;

        self.state = AgentState::Loaded {
            adapter: adapter.into(),
        };
        Ok(())
    }

    /// Process a search query through the TinyAgent LLMCompiler agent loop.
    ///
    /// New shape after Phase 1-5 of the integration:
    ///
    /// 1. Build planner system prompt via `agent::prompt::planner_system_prompt`
    ///    (cached after first call; SHA-256 byte-equivalent to the Python eval)
    /// 2. Run a single planner inference call via `agent::inference`
    ///    (loads base GGUF + trained LoRA, greedy decoding, stop on
    ///    `<END_OF_PLAN>`)
    /// 3. Parse the raw output into a `Vec<PlanStep>` via `agent::plan::parse_plan`
    /// 4. Execute the plan sequentially via `agent::executor::execute_plan`,
    ///    resolving `$N` references against prior step observations
    /// 5. Run the joinner second pass to convert the executed plan into a
    ///    natural-language `Action: Finish(...)` reply (Phase 6, not yet
    ///    wired in)
    /// 6. Return the joinner answer in the `SearchResponse`
    ///
    /// For now this stub returns an empty response — the actual wiring
    /// of the InferenceEngine + plan + executor + joinner happens in
    /// Phase 6 once the joinner module exists.
    pub async fn search(&self, params: SearchParams) -> Result<SearchResponse, LupusError> {
        self.require_loaded()?;

        tracing::debug!("Search query: {:?} scope: {:?}", params.query, params.scope);

        // TODO(phase 6): wire the full agent loop:
        //   let raw = self.inference.infer(prompt::planner_system_prompt(),
        //                                   &format!("Question: {}", params.query),
        //                                   plan::MAX_PLANNER_TOKENS).await?;
        //   let plan = plan::parse_plan(&raw)?;
        //   let records = executor::execute_plan(&plan).await;
        //   let joinner_out = joinner::run_joinner(&self.inference, &params.query, &records).await?;
        //   Ok(SearchResponse {
        //       text_answer: Some(joinner_out.answer),
        //       plan: Some(records.iter().map(...).collect()),
        //       results: harvest_results(&records),
        //   })

        Ok(SearchResponse { results: vec![] })
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
