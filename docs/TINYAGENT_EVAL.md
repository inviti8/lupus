# TinyAgent As-Is Evaluation Plan

Empirically determine what `squeeze-ai-lab/TinyAgent-1.1B` (Berkeley AI Research's edge function-calling model) can do **with no fine-tuning**, when given the Lupus daemon's specific tool surface in its prompt.

This document is the source of truth for the TinyAgent alpha. Execute it phase by phase. Each phase has a clear deliverable and a clear next step.

---

## Critical premise mismatch — read first

The current Lupus daemon scaffold assumes TinyAgent emits JSON-wrapped function calls in delimiter markers:

```
<|function_call|>{"name": "search_local_index", "arguments": {"query": "wolves"}}<|end_function_call|>
```

This is what `daemon/src/agent.rs:19-20` defines and what `daemon/src/tools/mod.rs::parse_tool_calls()` expects. **TinyAgent does not output that format.**

Per the BAIR blog and the TinyAgent GitHub repo, the model was trained to emit **LLMCompiler-style numbered plans**:

```
1. search_local_index(query="wolves")
2. fetch_page(url=$1)
3. join()
```

Numbered Python-like steps. `$N` variables reference the output of step N. Plans terminate in `join()` (or `join_finish()` / `join_replan()`).

This means several things are currently wrong in the repo:

- `daemon/src/tools/mod.rs::parse_tool_calls()` will never match real TinyAgent output
- `daemon/src/tools/mod.rs::system_prompt()` emits the wrong tool-spec format (JSON schema vs Python signature)
- The `assistant_response` field in `datasets/search/examples/knowledge_aware.jsonl` uses the wrong format
- The agent loop in `daemon/src/agent.rs` would silently fail to dispatch any tool calls

**Phase 1 of this plan exists to confirm or refute the format premise empirically.** Don't fix anything in the daemon until Phase 1 has produced actual model output to compare against.

---

## Goal and success criteria

**Goal**: a clear, evidence-backed answer to "does TinyAgent-1.1B, with our 6 tools described in its native prompt format, produce correct plans for 22 curated Lupus queries?" with a specific next-step decision attached.

**Success looks like one of these outcomes:**

- **Ship as-is**: green metrics (tool selection ≥ 80%, arg shape ≥ 90%, hallucination ≤ 2%) → drop the training plan, integrate into the daemon directly
- **Light prompt-engineering**: yellow metrics → add 2-5 in-context examples to the planner prompt, retest, ship
- **Small targeted LoRA**: specific failure mode (hallucinations OR multi-step) → train a small adapter on a few hundred targeted examples
- **Different base model**: red metrics → try TinyAgent-7B or Qwen2.5-Coder-1.5B before any training campaign

**Total budget**: ~6-9 hours of work, $0 expected cost (under $2 worst case if local inference fails and we fall back to a pod).

---

## Critical files in this repo

Read these before starting any phase:

| File | What to look for |
|---|---|
| `daemon/src/tools/mod.rs` | `schemas()`, `system_prompt()`, `parse_tool_calls()`, `FUNC_CALL_START/END` constants |
| `daemon/src/tools/{search_subnet,search_local,fetch_page,extract_content,scan_security,crawl_index}.rs` | Each `schema()` function — the 6 tool defs the eval must render into TinyAgent's native format |
| `daemon/src/agent.rs` | Lines 19-20 (marker constants), lines 89-110 (search loop stub) |
| `base/config.yaml` | Confirms base model id and that GGUF Q4_K_M is the export target |
| `tools/test_security_model.py` | Structural template for the new eval script: auto-detect model path, Windows mmap fallback, OK/X markers, summary counts, non-zero exit on failure |
| `datasets/search/examples/knowledge_aware.jsonl` | Source of test queries (61 entries). The `expected_tool_calls` field is valid ground truth; the `assistant_response` field uses the wrong format and should be ignored |

---

## External research to do before writing code

Spend ~30 minutes confirming these. The plan hinges on them.

1. **`https://github.com/SqueezeAILab/TinyAgent`** — clone or browse. Authoritative files:
   - `src/tiny_agent/prompts.py` — full planner prompt template including the `{tool_descriptions}` and `{examples}` placeholders. **Lift this verbatim.**
   - `src/tiny_agent/tiny_agent_tools.py` — how `TinyAgentTool` objects render into the prompt (Python signature with docstring)
   - `src/tiny_agent/llm_compiler/` — planner + joiner reference implementation
   - At least one complete example input/output to know exactly what "correct" looks like
2. **`https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/`** — worked example of a plan with `$1, $2, join()` so the eval scorer has a reference grammar
3. **`https://huggingface.co/squeeze-ai-lab/TinyAgent-1.1B-GGUF`** — confirm file list. Expect `tinyagent-1.1b.Q4_K_M.gguf` around 700 MB. Use the official GGUF, not a community quant
4. **`https://arxiv.org/abs/2409.00608`** sections 4-5 — BAIR's evaluation protocol, mirror their metric definitions where sensible
5. **`https://gorilla.cs.berkeley.edu/leaderboard.html`** — skim BFCL metric names (AST match, exec match, relevance) so our metrics line up with the field

---

## Phase 1: Verify the prompt-format match

**Goal**: one file, `tools/tinyagent_prompt_probe.py`, run locally, that loads the model and asks it one trivial query with TinyAgent's native prompt template and prints the raw output. **No scoring, just "what does the model actually produce."**

**Steps**:

1. Download the official GGUF:
   ```bash
   pip install huggingface_hub
   huggingface-cli download squeeze-ai-lab/TinyAgent-1.1B-GGUF tinyagent-1.1b.Q4_K_M.gguf --local-dir dist/tinyagent
   ```
2. Lift the planner prompt template verbatim from `TinyAgent/src/tiny_agent/prompts.py`
3. Render our 6 Lupus tools into **TinyAgent's native format** (Python-signature-style with docstring):
   ```python
   def search_local_index(query: str, top_k: int = 10) -> dict:
       """Search the local semantic index for previously visited pages"""
   ```
   Write a small `render_lupus_tools_as_tinyagent()` helper. This lives in the eval script, not the daemon
4. Feed a baby query: `"Find pages about wolves in the local index"`
5. Print the raw model output verbatim
6. Inspect: does it emit `1. search_local_index(query="wolves") 2. join()`? If yes, premise confirmed. If it emits something else (JSON, ReAct, Hermes-style), document exactly what

**Output of Phase 1**: a written note describing the actual observed format, plus a note on **how far our repo's assumed format is from reality**. If the gap is real, file a separate work item to update `FUNC_CALL_START/END` and `parse_tool_calls()` — but do not make that change in this phase. This is read-only planning + probing.

---

## Phase 2: Set up inference — local first

**Recommendation**: local llama.cpp via `llama-cpp-python`.

**Reasons**:
- TinyAgent 1.1B at Q4_K_M is ~700 MB
- The GGUF format matches the daemon's future runtime
- Avoids the Windows safetensors mmap issue that hit the security model
- No pod spend needed for the eval phase

**Commands** (Windows, bash):

```bash
pip install llama-cpp-python huggingface_hub
huggingface-cli download squeeze-ai-lab/TinyAgent-1.1B-GGUF tinyagent-1.1b.Q4_K_M.gguf --local-dir dist/tinyagent
python tools/tinyagent_prompt_probe.py   # Phase 1 smoke
python tools/eval_tinyagent.py            # Phase 4 full eval
```

**Fallback order if `llama-cpp-python` build fails on Windows** (common; needs C++ build tools):

1. Install prebuilt wheel:
   ```bash
   pip install llama-cpp-python --prefer-binary --extra-index-url https://abetlen.github.io/llama-cpp-python/whl/cpu
   ```
2. Fallback: transformers + safetensors with the mmap workaround pattern from `tools/test_security_model.py` — download the non-GGUF `squeeze-ai-lab/TinyAgent-1.1B` repo, load via `safetensors.torch.load_file()`
3. Last resort (only if local breaks for >1 hour): `python tools/runpod_deploy.py --on-demand`, attach network volume `b51y8tev3v`, `tmux new -s lupus`, rerun the same eval script. Cost ~$0.59/hr for ~1 hour, under $1 total

---

## Phase 3: Test query set

22 hand-curated queries organized by the behavior each one probes. Each case is `(id, query, expected_tools_unordered_set, must_be_multi_step_bool, notes)`. Ground truth is hand-labeled by the executor against the 6 tool schemas; **no TinyAgent output is used to label queries**.

### Single-tool — `search_local_index` (5)
1. "Find pages about wolves in my local index"
2. "What did I save about Anishinaabe folklore?"
3. "Search my local history for rust borrow checker explanations"
4. "Any pages mentioning IPFS content routing?"
5. "Show me saved articles about wool felting"

### Single-tool — `search_subnet` (3)
6. "Search the hvym cooperative for weaving datapods"
7. "Find datapods about open-source 3D printing"
8. "Is there a subnet entry for Lepus browser docs?"

### Single-tool — `fetch_page` (2)
9. "Fetch https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/"
10. "Get the content of hvym://cooperative/weaving/intro"

### Single-tool — `scan_security` (2)
11. "Is https://paypa1-secure.support/login.php safe?" (HTML not provided → model should either pick `fetch_page` first or call `scan_security` with empty html — document which)
12. "Check https://github.com/inviti8/lupus for threats"

### Single-tool — `crawl_index` (1)
13. "Add https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/ to my index"

### Multi-step (5)
14. "Summarize the BAIR TinyAgent blog post" → `fetch_page` + `extract_content` (summary format)
15. "Fetch https://example.com/article.html and tell me if it's dangerous" → `fetch_page` + `scan_security`
16. "Find a cooperative datapod about weaving and save it to my index" → `search_subnet` + `crawl_index`
17. "Look up wolves in my local index, then fetch the first result in full" → `search_local_index` + `fetch_page` (`$1` dependency — **critical LLMCompiler test**)
18. "Fetch the Lupus GitHub README, extract its summary, and scan it for threats" → 3-tool chain

### No-tool / abstention (2)
19. "What is 2+2?" → expected: `join()` with a direct answer, no tool calls. Hallucinated tool use here is a red flag
20. "Who are you?" → same

### Adversarial (2, to catch hallucination)
21. "Email my wife that I'll be late" → model has no email tool. Expected: abstain via `join()` or `join_replan()`. Calling a fabricated `send_email` is the headline failure mode
22. "Open a terminal and run `ls`" → expected: abstain

Total: 22 queries. Small enough to iterate fast, broad enough to exercise every tool at least twice.

---

## Phase 4: Eval script outline

`tools/eval_tinyagent.py` — structure mirrors `tools/test_security_model.py`.

```python
# --- constants ---
MODEL_GGUF = "dist/tinyagent/tinyagent-1.1b.Q4_K_M.gguf"
TINYAGENT_PLANNER_PROMPT = """<verbatim from upstream prompts.py>"""
LUPUS_TOOLS = [
    # 6 tuples: (name, py_signature, docstring) derived from daemon/src/tools/*.rs
]
TEST_CASES = [
    # the 22 from Phase 3, each with expected_tools: set[str], multi_step: bool, allow_abstain: bool
]

# --- render tools in TinyAgent native format ---
def render_tools(tools) -> str:
    # produces:
    # def search_local_index(query: str, top_k: int = 10) -> dict:
    #     """Search the local semantic index..."""
    ...

# --- load ---
from llama_cpp import Llama
llm = Llama(model_path=MODEL_GGUF, n_ctx=4096, n_gpu_layers=0, verbose=False)

# --- run one ---
def run(query):
    prompt = TINYAGENT_PLANNER_PROMPT.format(
        tool_descriptions=render_tools(LUPUS_TOOLS),
        examples="",
        query=query,
    )
    out = llm(prompt, max_tokens=512, stop=["<|eot_id|>", "\n\n\n"])
    return out["choices"][0]["text"]

# --- parse LLMCompiler plan ---
def parse_plan(text) -> list[ParsedCall]:
    # regex over lines matching r'^\s*(\d+)\.\s*(\w+)\((.*)\)\s*$'
    # capture name and raw arg string; try ast.literal_eval on args wrapped in dict
    # also detect join() / join_finish() / join_replan()
    ...

# --- score one ---
def score(case, parsed):
    return {
        "valid_syntax": bool(parsed) or case.allow_abstain,
        "tool_selection": {p.name for p in parsed if p.name not in JOIN_NAMES} == case.expected_tools,
        "arg_shape_valid": all(matches_schema(p, LUPUS_SCHEMAS) for p in parsed),
        "hallucinated": any(p.name not in LUPUS_TOOL_NAMES | JOIN_NAMES for p in parsed),
        "multi_step_ok": (len([p for p in parsed if p.name not in JOIN_NAMES]) >= 2) == case.multi_step,
        "abstained_correctly": case.allow_abstain and not any(p.name not in JOIN_NAMES for p in parsed),
    }

# --- report ---
# Per-case: ok / X / ? marker, query, raw output (trimmed), parsed plan, per-metric booleans.
# Summary table across metrics. Exit 0 if all hard metrics pass, 2 otherwise.
```

**Save raw outputs to `eval/tinyagent_runs/<timestamp>.jsonl`** so the executor can diff runs after any prompt-format tweak without re-running every time.

---

## Phase 5: Metrics, thresholds, interpretation

Six metrics, computed per case then aggregated:

| Metric | Definition |
|---|---|
| **Syntactic validity rate** | Fraction where the output parses into a valid plan (or a valid abstention). "Does the model produce the output shape at all" |
| **Tool selection accuracy** | Fraction where the unordered set of called tool names equals the expected set (excluding `join*`). The single headline number |
| **Argument shape validity** | Fraction of tool calls whose args can be coerced into the `ToolSchema` required fields without type errors. Distinguishes "picked right tool, wrong args" from "picked wrong tool" |
| **Hallucinated tool rate** | Fraction of total tool calls that reference a name not in our 6-tool surface. Must be near zero; anything above 5% is a red flag |
| **Multi-step correctness** | For the 5 multi-step cases: did the model emit ≥2 calls *and* use `$N` variable references to chain them? The LLMCompiler-specific capability |
| **Abstention correctness** | For the 4 no-tool / adversarial cases: did the model correctly produce `join_finish(...)` without inventing tools |

**Thresholds** (informed by BFCL norms and BAIR's own 80% claim):

| Color | Tool selection | Arg shape | Hallucination | Multi-step | Abstention |
|---|---|---|---|---|---|
| 🟢 Green | ≥ 80% | ≥ 90% | ≤ 2% | ≥ 60% | ≥ 75% |
| 🟡 Yellow | 60-80% | 75-90% | 2-5% | 40-60% | 50-75% |
| 🔴 Red | < 60% | < 75% | > 5% | < 40% | < 50% |

---

## Phase 6: Decision tree for next steps

Branches are evaluated **in order**; take the first match.

1. **Syntactic validity < 70%** → prompt format is wrong. Re-verify Phase 1 against the TinyAgent repo source. Do not interpret any other metric until this clears 90%. No training decision yet.
2. **All green** (validity ≥ 90%, selection ≥ 80%, args ≥ 90%, hallucinations ≤ 2%) → **ship as-is.** Next phase is daemon integration: port `render_tools()` and the LLMCompiler parser from the eval script into `daemon/src/tools/mod.rs` + `daemon/src/agent.rs`, replace the JSON `<|function_call|>` assumption with LLMCompiler plan parsing, drop the training plan entirely. The `knowledge_aware.jsonl` dataset is not needed.
3. **Tool selection 60-80%, otherwise green** → model understands function calling but confuses our tools (likely `search_subnet` vs `search_local_index`). Add 2-3 in-context examples to the planner prompt (TinyAgent's prompt supports an `{examples}` slot natively) and re-run the eval. No training yet. If examples push it above 80%, ship with examples baked into `system_prompt()`.
4. **Hallucinated tool rate > 5% OR abstention < 50%** → model is making up tools or refusing to abstain. First try ToolRAG: pull `squeeze-ai-lab/TinyAgent-ToolRAG` to filter the tool list per query; this is what BAIR trained for. If still bad, this is a small-LoRA case: fine-tune on `knowledge_aware.jsonl` plus 100-200 hand-written negative examples (adversarial queries with `join_finish()` targets). Use the LoRA config already in `base/config.yaml`.
5. **Multi-step correctness < 40% but single-tool selection ≥ 80%** → the model can pick tools but can't chain them with `$N`. Add 3-5 multi-step in-context examples to the prompt and re-run. If that doesn't fix it, it's a small-LoRA case on a multi-step-specific dataset.
6. **Tool selection < 60% AND arg shape < 75%** → model is fundamentally struggling with our tool surface. Options in order of cost:
   a. Try the **TinyAgent-7B** variant if it fits on a 4090 — same prompt format, much stronger
   b. Consider **Qwen2.5-Coder-1.5B** as a function-calling base (already in-family with the security model)
   c. **Only then** consider a full LoRA training campaign on a hand-curated ~500-example dataset
7. **The repo's JSON marker format was the only gap** (the rest of the numbers are green once we swap the prompt/parser) → file a separate work item to fix `daemon/src/agent.rs` and `daemon/src/tools/mod.rs` markers and parser; eval already validates the replacement.

---

## Time and cost estimate

| Phase | Time | Cost |
|---|---|---|
| Phase 1 — Format probe | 1-2 hours | $0 (local) |
| Phase 2 — Local inference setup | 0.5-2 hours | $0; <$1 if pod fallback |
| Phase 3 — Write 22 test cases with expected tools | 1 hour | $0 |
| Phase 4 — Write `tools/eval_tinyagent.py` | 2-3 hours | $0 |
| Phase 5 — Run eval, interpret, write findings note | 1 hour | $0 |
| **Total** | **~6-9 hours** | **$0 expected, <$2 worst case** |

One afternoon, one session. After this, the next planning pass can make an evidence-based training-or-ship decision.
