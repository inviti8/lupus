//! Lupus daemon inference module — wraps `llama-cpp-2` to run the trained
//! TinyAgent planner LoRA against the base GGUF.
//!
//! Loads the base model + LoRA once at daemon startup, then runs a
//! greedy planner inference per request. Uses the GGUF's embedded chat
//! template (the flat-concat custom one extracted in
//! `training/train_planner.py::TINYAGENT_CHAT_TEMPLATE`) so the prompt
//! format matches what the LoRA was trained against.
//!
//! Stop-string handling is manual: `llama-cpp-2`'s sampler API doesn't
//! support string-stop sentinels, so we detokenize each new token,
//! append to a running output, and check for stop-string suffixes after
//! each step. All our stop strings are pure ASCII so byte-level matching
//! is exact.
//!
//! For v1 the daemon serializes inference requests through a single
//! `InferenceEngine` (held in an outer `Mutex` by `Agent`). Concurrent
//! requests are queued, not parallelized. Adding parallelism would
//! require multiple contexts and is deferred to a later phase.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::model::LlamaLoraAdapter;
use llama_cpp_2::sampling::LlamaSampler;

use crate::error::LupusError;

/// Stop strings for the **planner** pass (with LoRA). Mirrors
/// `stop=[END_OF_PLAN, "<|eot_id|>", "</s>", "###"]` in
/// `tools/eval_tinyagent.py` and `tools/tinyagent_prompt_probe.py`.
/// All entries are pure ASCII so byte-level matching is safe.
pub const PLANNER_STOP_STRINGS: &[&str] = &["<END_OF_PLAN>", "<|eot_id|>", "</s>", "###"];

/// Stop strings for the **joinner** pass (without LoRA). Does NOT
/// include `"###"` because `OUTPUT_PROMPT_FINAL` uses `"###\n"` as a
/// separator between in-context examples. If the model emits `"###"`
/// as its first generated tokens (which it naturally does after
/// seeing the `"###\n"` pattern at the end of each example), the
/// planner stop set would truncate the output to empty — producing
/// the `joinner_raw=""` symptom. The joinner only needs EOG / EOS /
/// EOT markers.
pub const JOINNER_STOP_STRINGS: &[&str] = &["<|eot_id|>", "</s>"];

/// LoRA scale matching the eval default. `llama-cpp-python` passes 1.0
/// implicitly when no `lora_scale` is given to `Llama(...)`.
pub const LORA_SCALE: f32 = 1.0;

/// Maximum tokens to generate per planner call. The longest plan we've
/// observed in the 22-case eval is ~80 tokens; 512 is comfortable headroom.
pub const MAX_PLANNER_TOKENS: usize = 512;

/// Default context size in tokens. Matches the eval's `n_ctx=4096` so the
/// rendered system prompt (~1550 tokens) plus user query plus generation
/// budget all fit comfortably.
pub const DEFAULT_N_CTX: u32 = 4096;

/// Send wrapper around `LlamaLoraAdapter`.
///
/// `llama-cpp-2` 0.1.143 declares `unsafe impl Send for LlamaModel` (and
/// for `LlamaBackend`/`LlamaContextParams`) but is missing the same for
/// `LlamaLoraAdapter`. The underlying llama.cpp adapter struct is
/// loaded once via `llama_adapter_lora_init`, never modified after
/// construction, and only attached to contexts (which is itself a
/// thread-serialized operation in our usage). It IS safe to send across
/// threads, but the Rust type system can't see that without an explicit
/// declaration.
///
/// We serialize all access to the engine via the outer `Mutex<InferenceEngine>`
/// in `Agent`, so the wrapper is sound: only one thread ever touches
/// the adapter at a time.
struct SendableLora(LlamaLoraAdapter);

// SAFETY: see SendableLora doc comment. The wrapped LlamaLoraAdapter is
// initialized once, never mutated after construction, and accessed only
// through the engine's serialized methods. Sound for our usage.
unsafe impl Send for SendableLora {}

/// Inference engine that owns the loaded base model + LoRA adapter for
/// the process lifetime. Constructed once at daemon startup via
/// [`InferenceEngine::load`] and reused for every planner request via
/// [`InferenceEngine::infer_blocking`]. Concurrent requests serialize
/// through the outer `Mutex` wrapping this struct in `Agent`.
pub struct InferenceEngine {
    /// Backend handle. Held as Arc so spawn_blocking closures can clone
    /// without taking ownership; we never actually share the backend
    /// across threads in v1 because all inference goes through the same
    /// engine instance.
    #[allow(dead_code)] // kept for future multi-engine designs; needed to keep backend alive
    backend: Arc<LlamaBackend>,

    /// The loaded base GGUF. Borrowed by the per-call `LlamaContext`.
    model: LlamaModel,

    /// The trained search planner LoRA adapter, wrapped to be `Send`.
    /// Held mutably so we can attach it to a fresh context per
    /// inference call. See [`SendableLora`] for the safety argument.
    search_lora: SendableLora,
}

impl InferenceEngine {
    /// Load the base GGUF and attach the search LoRA adapter. Called
    /// once at daemon startup. The backend is initialized lazily.
    pub fn load(model_path: &Path, lora_path: &Path) -> Result<Self, LupusError> {
        if !model_path.exists() {
            return Err(LupusError::ModelLoadFailed(format!(
                "base GGUF not found at {}",
                model_path.display()
            )));
        }
        if !lora_path.exists() {
            return Err(LupusError::ModelLoadFailed(format!(
                "search LoRA adapter not found at {}",
                lora_path.display()
            )));
        }

        tracing::info!("Initializing llama.cpp backend...");
        let backend = LlamaBackend::init().map_err(|e| {
            LupusError::ModelLoadFailed(format!("LlamaBackend init failed: {e}"))
        })?;
        let backend = Arc::new(backend);

        tracing::info!("Loading base GGUF: {}", model_path.display());
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| LupusError::ModelLoadFailed(format!("base GGUF load failed: {e}")))?;
        tracing::info!(
            "Base model loaded: {} params ({} MB)",
            model.n_params(),
            model.size() / (1024 * 1024)
        );

        tracing::info!("Loading search LoRA adapter: {}", lora_path.display());
        let search_lora = model.lora_adapter_init(lora_path).map_err(|e| {
            LupusError::ModelLoadFailed(format!("LoRA adapter init failed: {e}"))
        })?;
        tracing::info!("Search LoRA adapter loaded");

        Ok(Self {
            backend,
            model,
            search_lora: SendableLora(search_lora),
        })
    }

    /// Run a single inference call. Builds a fresh `LlamaContext`,
    /// optionally attaches the search LoRA, formats the `(system, user)`
    /// messages via the GGUF's embedded chat template, runs greedy
    /// sampling token-by-token until a stop string suffix is detected
    /// or `max_tokens` is reached, returns the accumulated string with
    /// the stop suffix stripped.
    ///
    /// `use_lora`:
    /// - `true` for **planner** calls — attaches the trained search LoRA
    ///   at scale [`LORA_SCALE`]. The trained LoRA was fine-tuned on
    ///   354 (query, plan) pairs and is what makes the planner pick the
    ///   right Lupus tools 95.5% of the time.
    /// - `false` for **joinner** calls — leaves the context's LoRA
    ///   slot empty so the base TinyAgent model produces the
    ///   `Action: Finish(...)` output format BAIR trained it to emit.
    ///   Running the joinner with the planner LoRA attached risks the
    ///   LoRA biasing the model toward LLMCompiler plans instead.
    ///
    /// This is a synchronous, CPU-blocking call. Wrap in
    /// `tokio::task::spawn_blocking` from async contexts.
    ///
    /// Reference behavior: the equivalent Python call is
    /// `llm.create_chat_completion(messages=[{system}, {user}],
    /// temperature=0.0, stop=[END_OF_PLAN, ...])` from
    /// `tools/tinyagent_prompt_probe.py`.
    pub fn infer_blocking(
        &mut self,
        system_prompt: &str,
        user_query: &str,
        max_tokens: usize,
        use_lora: bool,
        stop_strings: &[&str],
    ) -> Result<String, LupusError> {
        // Fresh context for this call. Cheap relative to inference itself.
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(DEFAULT_N_CTX))
            .with_n_threads(num_threads());
        let mut context = self.model.new_context(&self.backend, ctx_params).map_err(|e| {
            LupusError::Inference(format!("context creation failed: {e}"))
        })?;

        if use_lora {
            // Attach the trained planner LoRA. Joinner calls skip this so
            // the base model handles the Finish output format.
            context
                .lora_adapter_set(&mut self.search_lora.0, LORA_SCALE)
                .map_err(|e| {
                    LupusError::Inference(format!("lora_adapter_set failed: {e}"))
                })?;
        }

        // Render the chat template manually. TinyAgent's GGUF embeds a
        // custom Jinja template (see training/train_planner.py::
        // TINYAGENT_CHAT_TEMPLATE) that does flat concatenation:
        //     {system}{user}{assistant}\n
        // with NO role markers and NO separators. llama.cpp's
        // llama_chat_apply_template C++ function only supports a hardcoded
        // set of well-known templates and rejects custom Jinja with FFI
        // error -1 — llama-cpp-python sidesteps this by rendering Jinja
        // in Python before calling llama.cpp. Since our template is just
        // string concatenation, we replicate it in two lines without
        // pulling in a Jinja crate.
        //
        // For just system + user (no assistant turn yet — the model is
        // about to generate it), the rendered prompt is `{system}{user}`.
        // This was verified earlier in the eval phase by inspecting the
        // GGUF metadata directly. The byte-equivalence test in
        // agent::prompt enforces that `system_prompt` matches the Python
        // canonical bytes; we trust the same of `user_query` since it's
        // the verbatim user input.
        let rendered = format!("{system_prompt}{user_query}");

        // Tokenize. The TinyAgent GGUF has `tokenizer.ggml.add_bos_token = true`
        // so we use AddBos::Always to mirror what llama-cpp-python does.
        let prompt_tokens = self
            .model
            .str_to_token(&rendered, AddBos::Always)
            .map_err(|e| LupusError::Inference(format!("tokenization failed: {e}")))?;
        let prompt_len = prompt_tokens.len();
        if prompt_len == 0 {
            return Err(LupusError::Inference(
                "prompt tokenized to zero tokens".into(),
            ));
        }

        // Feed the prompt into the context. `logits_all=false` means
        // only the LAST token in the sequence gets logits, which is what
        // we want for greedy generation (we sample the next token from
        // the position right after the prompt).
        let mut batch = LlamaBatch::new(prompt_len, 1);
        batch.add_sequence(&prompt_tokens, 0, false).map_err(|e| {
            LupusError::Inference(format!("batch add_sequence failed: {e}"))
        })?;
        context.decode(&mut batch).map_err(|e| {
            LupusError::Inference(format!("prompt decode failed: {e}"))
        })?;

        // Greedy sampler — temperature 0 equivalent. The eval uses
        // temperature=0.0 in `create_chat_completion`, which collapses
        // to greedy argmax sampling.
        let mut sampler = LlamaSampler::chain_simple([LlamaSampler::greedy()]);

        // Sampling loop. Accumulate output bytes; check stop strings after
        // each new token; break on EOG, stop string, or max_tokens.
        let mut output_bytes: Vec<u8> = Vec::with_capacity(256);
        let mut n_cur: i32 = prompt_len.try_into().map_err(|_| {
            LupusError::Inference("prompt length exceeds i32::MAX".into())
        })?;

        for _ in 0..max_tokens {
            // Sample from the logits at the position of the last token in
            // the most recent batch (which is always at index n_tokens - 1).
            let new_token = sampler.sample(&context, batch.n_tokens() - 1);
            sampler.accept(new_token);

            if self.model.is_eog_token(new_token) {
                break;
            }

            // Detokenize this single token to its UTF-8 byte representation.
            // 16 bytes is enough for any single token from a llama-family
            // BPE vocab; the API auto-grows on InsufficientBufferSpace.
            let token_bytes = self
                .model
                .token_to_piece_bytes(new_token, 16, /* special */ false, None)
                .map_err(|e| LupusError::Inference(format!("token_to_piece_bytes: {e}")))?;
            output_bytes.extend_from_slice(&token_bytes);

            // Stop string check. Our stop strings are all ASCII so byte-
            // level search is exact and we can truncate cleanly.
            if let Some(stop_at) = stop_string_index(&output_bytes, stop_strings) {
                output_bytes.truncate(stop_at);
                break;
            }

            // Feed this single new token back as the next decode step.
            batch.clear();
            batch.add(new_token, n_cur, &[0], true).map_err(|e| {
                LupusError::Inference(format!("batch add failed: {e}"))
            })?;
            n_cur += 1;
            context.decode(&mut batch).map_err(|e| {
                LupusError::Inference(format!("decode failed: {e}"))
            })?;
        }

        // Convert accumulated UTF-8 bytes to a String. Lossy in case the
        // last token left a partial UTF-8 sequence (rare but possible).
        Ok(String::from_utf8_lossy(&output_bytes).into_owned())
    }
}

/// Find the byte index of the earliest stop string match in the byte
/// stream. Returns `None` if no stop string occurs anywhere.
///
/// The stop strings in [`STOP_STRINGS`] are all pure ASCII, so byte-level
/// search via `windows()` matching gives exact results. We do a linear
/// scan over each stop string and pick the earliest match.
fn stop_string_index(haystack: &[u8], stops: &[&str]) -> Option<usize> {
    let mut earliest: Option<usize> = None;
    for stop in stops {
        let needle = stop.as_bytes();
        if needle.is_empty() || needle.len() > haystack.len() {
            continue;
        }
        if let Some(idx) = haystack
            .windows(needle.len())
            .position(|w| w == needle)
        {
            earliest = Some(match earliest {
                Some(prev) if prev < idx => prev,
                _ => idx,
            });
        }
    }
    earliest
}

/// Pick a sensible thread count for inference. Defaults to half the
/// available logical CPUs (typically the physical core count) which is
/// what llama.cpp recommends for CPU inference without HT contention.
fn num_threads() -> i32 {
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let half = (parallelism / 2).max(1);
    i32::try_from(half).unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_string_finds_first() {
        let text = b"1. join()<END_OF_PLAN>";
        assert_eq!(
            stop_string_index(text, &["<END_OF_PLAN>", "</s>"]),
            Some(9)
        );
    }

    #[test]
    fn stop_string_returns_none() {
        let text = b"1. search_local_index";
        assert_eq!(stop_string_index(text, &["<END_OF_PLAN>", "</s>"]), None);
    }

    #[test]
    fn stop_string_picks_earliest() {
        let text = b"abc</s>def<END_OF_PLAN>ghi";
        assert_eq!(
            stop_string_index(text, &["<END_OF_PLAN>", "</s>"]),
            Some(3)
        );
    }

    #[test]
    fn stop_string_handles_empty_haystack() {
        assert_eq!(stop_string_index(b"", &["<END_OF_PLAN>"]), None);
    }

    #[test]
    fn stop_string_handles_short_haystack() {
        assert_eq!(stop_string_index(b"abc", &["<END_OF_PLAN>"]), None);
    }

    #[test]
    fn num_threads_is_at_least_one() {
        assert!(num_threads() >= 1);
    }
}
