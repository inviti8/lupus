# TinyAgent Phase 1 Findings — Format Probe

**Date**: 2026-04-07
**Probe script**: `tools/tinyagent_prompt_probe.py`
**Raw run**: `eval/tinyagent_runs/20260407-220539-phase1-probe.log`
**Model**: `dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf` (637 MB, official BAIR GGUF)
**Runtime**: `llama-cpp-python` 0.3.20, CPU only, `n_ctx=4096`, `n_gpu_layers=0`

---

## Result: LLMCompiler premise CONFIRMED

For the trivial query `"Find pages about wolves in my local index"` with the
6 Lupus tools rendered in TinyAgent's native Python-signature format and the
verbatim LLMCompiler planner system prompt (lifted from
`dist/tinyagent-source/src/llm_compiler/planner.py::generate_llm_compiler_prompt`),
the model emits — on **both** the chat-completion path and the raw text-completion
path — the same plan:

```
1. search_local_index("wolves", 10)
2. join()
```

Inference time: ~13 s per probe on CPU. Both paths produce identical output
at `temperature=0.0`. The leading `.` character is just whitespace before the
plan; the structured part is exactly what BAIR's blog and the upstream
`prompts.py` examples lead us to expect.

This is **not** the format the daemon currently assumes.

---

## Gap analysis: what the daemon assumes vs reality

| Concern | Daemon currently does | Reality (TinyAgent) |
|---|---|---|
| Output delimiters | `<\|function_call\|>...<\|end_function_call\|>` markers around a JSON blob (`daemon/src/agent.rs:19-20`) | No delimiters. Plain text. Numbered Python-like steps |
| Tool call shape | `{"name": "...", "arguments": {...}}` JSON | `1. tool_name(arg1, arg2)` — positional, **no named args** (per `LUPUS_CUSTOM_INSTRUCTIONS`) |
| Multi-call dependencies | Sequential — model is told to "wait for the result before continuing" (`daemon/src/tools/mod.rs::system_prompt()`) | Single planner pass. Plans use `$N` to reference the output of step N (e.g. `extract_content("$1", "summary")`) and the executor resolves dependencies |
| Termination | Implicit (next assistant turn) | Explicit `join()` (or `join_finish(...)` / `join_replan(...)`) followed by `<END_OF_PLAN>` sentinel |
| Tool rendering in system prompt | JSON Schema `parameters` block per tool (`mod.rs::system_prompt()` with `serde_json::to_string_pretty`) | Python signature with bullet-list docstring, e.g. `search_local_index(query: str, top_k: int) -> dict\n - Search the local semantic index...` |
| Parse function | `parse_tool_calls()` finds the JSON markers and `serde_json::from_str` (`mod.rs::parse_tool_calls()`) | Needs a regex `(\d+)\. (\w+)\((.*)\)` and `ast.literal_eval`-style positional arg parsing — see upstream `dist/tinyagent-source/src/llm_compiler/output_parser.py::ACTION_PATTERN` |
| Examples in prompt | None | TinyAgent's planner prompt has a dedicated examples slot (`{example_prompt}`) and the model is clearly tuned to expect 3-4 worked examples; outputs degrade without them |

Net: every component the daemon scaffold writes for tool dispatch is wrong
in shape. The good news is the **agent loop semantics** are simpler than
expected — TinyAgent does the whole plan in one shot, the executor runs the
DAG, and only `join()` triggers a second LLM call.

---

## Items NOT changed in this phase (per plan)

Per Phase 1 of `docs/TINYAGENT_EVAL.md`: this is read-only probing, no daemon
edits. The following work items are deferred until after Phase 5 metrics
clear (Phase 6 decision tree branch 7):

1. **`daemon/src/agent.rs:19-20`** — delete `FUNC_CALL_START` and `FUNC_CALL_END`
   constants. They have no analog in LLMCompiler output.
2. **`daemon/src/tools/mod.rs::system_prompt()`** — rewrite to emit:
   - The verbatim LLMCompiler scaffold from
     `dist/tinyagent-source/src/llm_compiler/planner.py::generate_llm_compiler_prompt`
   - Each tool rendered as a Python signature + bullet-list docstring
     (mirror `tinyagent_prompt_probe.py::LUPUS_TOOLS`)
   - The `<END_OF_PLAN>` sentinel
   - 3-4 in-context examples lifted from the verified eval prompt
3. **`daemon/src/tools/mod.rs::parse_tool_calls()`** — replace JSON-marker
   parsing with LLMCompiler plan parsing:
   - Regex `^\s*(\d+)\.\s*(\w+)\((.*)\)\s*$` per line
   - Positional arg parsing via something like Rust's `litrs` crate or a
     hand-rolled `parse_args()` (the upstream uses Python's `ast.literal_eval`)
   - Detect `join()` / `join_finish(...)` / `join_replan(...)` as terminators
   - Resolve `$N` references against prior step results
4. **`daemon/src/agent.rs::search_loop` (lines 89-110)** — rewrite from a
   ReAct-style "ask, parse one call, run it, repeat" loop into a
   plan-then-execute-DAG loop. Probably grow a small `Plan` struct with
   step `idx`, `tool_name`, `args` (raw and resolved), `dependencies`, and
   `result`. Execution layer can run independent steps in parallel.
5. **`datasets/search/examples/knowledge_aware.jsonl`** — the
   `assistant_response` field uses the old JSON marker format and is not
   usable as-is. The `expected_tool_calls` field is still valid as ground
   truth for the eval; the `assistant_response` would only matter if we
   later decided to fine-tune (Phase 6 branch 4 or 5), in which case it
   needs to be regenerated in LLMCompiler format.

---

## What this unlocks

Phase 2 (local inference setup) is **already done** as a side-effect of
this probe — `llama-cpp-python` 0.3.20 built from source on Windows
(took ~7 min, built via the prebuilt-wheel index then fell back to a
local build), the GGUF loaded fine in 3 s, and inference is ~13 s per
query at zero cost. No pod needed.

Next: Phase 3 (write 22 test queries — already specced in
`docs/TINYAGENT_EVAL.md` lines 142-184) and Phase 4 (write
`tools/eval_tinyagent.py`, structurally a clone of
`tools/test_security_model.py`).

The probe script `tools/tinyagent_prompt_probe.py` is the canonical reference
for the planner prompt rendering. The eval script should reuse
`build_planner_system_prompt()` and `LUPUS_TOOLS` directly, not redefine them.
