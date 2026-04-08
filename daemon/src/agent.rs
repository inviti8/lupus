//! TinyAgent‑based search agent with LoRA adapter hot‑swapping.
//!
//! Manages the search model lifecycle: load base weights, attach a LoRA
//! adapter (search or content), run the agent loop with tool calling.

pub mod prompt;

use crate::config::ModelsConfig;
use crate::error::LupusError;
use crate::protocol::{
    ComponentState, SearchParams, SearchResponse,
    SummarizeParams, SummarizeResponse,
};
use crate::tools;

/// Known LoRA adapters that can be hot‑swapped onto the base model.
pub const ADAPTER_SEARCH: &str = "search";
pub const ADAPTER_CONTENT: &str = "content";

/// TinyAgent function‑call markers (parsed from model output).
pub const FUNC_CALL_START: &str = "<|function_call|>";
pub const FUNC_CALL_END: &str = "<|end_function_call|>";

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

    /// Process a search query through the TinyAgent tool‑calling loop.
    ///
    /// 1. Build system prompt with available tool schemas
    /// 2. Present query to model
    /// 3. Parse tool calls from output
    /// 4. Execute tools, feed results back
    /// 5. Model generates final response
    pub async fn search(&self, params: SearchParams) -> Result<SearchResponse, LupusError> {
        self.require_loaded()?;

        tracing::debug!("Search query: {:?} scope: {:?}", params.query, params.scope);

        // Build the system prompt with tool schemas
        let _system_prompt = tools::system_prompt();

        // TODO: Run inference loop
        //   let prompt = format!("{}\n\nUser: {}", system_prompt, params.query);
        //   loop {
        //       let output = model.generate(&prompt, max_tokens)?;
        //       if let Some(call) = parse_tool_call(&output) {
        //           let result = tools::execute(&call.name, call.args, &tool_ctx).await?;
        //           prompt.push_str(&format_tool_result(&result));
        //       } else {
        //           break parse_search_results(&output);
        //       }
        //   }

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
