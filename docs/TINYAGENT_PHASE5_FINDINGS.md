# TinyAgent Phase 5 Findings — Eval & Decision

**Date**: 2026-04-07
**Eval script**: `tools/eval_tinyagent.py`
**Raw run**: `eval/tinyagent_runs/20260407-221537-eval.jsonl` (22 cases, 44 s total inference, $0)
**Model**: `dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf` (637 MB official BAIR GGUF)
**Inference**: `llama-cpp-python` 0.3.20, CPU only, `n_ctx=4096`, temperature 0
**Total cost**: $0 (Phase 2 local-inference recommendation worked first try)

---

## Headline metrics (Phase 5 thresholds)

| Metric | Value | Color | Threshold |
|---|---|---|---|
| Syntactic validity rate | 100.0% | 🟢 GREEN | ≥ 90% |
| Tool selection accuracy | **59.1%** | **🔴 RED** | ≥ 80% green / ≥ 60% yellow |
| Argument shape validity | 81.8% | 🟡 YELLOW | ≥ 90% green / ≥ 75% yellow |
| Hallucinated tool rate | **9.1%** | **🔴 RED** | ≤ 2% green / ≤ 5% yellow |
| Multi-step correctness | 60.0% | 🟢 GREEN | ≥ 60% green / ≥ 40% yellow |
| Abstention correctness | 75.0% | 🟢 GREEN | ≥ 75% green / ≥ 50% yellow |

**Hard pass (all metrics ok per case)**: 11 / 22 = 50%

---

## What worked

**LLMCompiler grammar is rock-solid.** Every single one of the 22 cases parsed
into a valid plan with a `join()` terminator. The model knows exactly how to
emit the output shape; there is no ambiguity. The `<END_OF_PLAN>` stop token
fires reliably.

**Search-tool selection is near-perfect** for the simple wording (cases 1-6, 8):
8 / 8 single-tool search queries got the right tool with the right args. The
model handles `top_k=10` defaults and string queries cleanly.

**`$N` dependency chaining works** — case 17 (the critical LLMCompiler test:
`search_local_index → fetch_page($1)`) produces a perfectly-formed two-step
plan. The model understands `$N` references natively.

**Abstention works most of the time** — 3 / 4 abstention cases
(2+2, "Who are you?", `ls` terminal) correctly emit a bare `join()` with no
tool calls.

---

## What broke and why (root-cause analysis)

I sat with the JSONL line-by-line. The 11 hard fails decompose into 4
distinct failure modes, in order of severity:

### 1. Hallucinated tools (2/22, 9.1% — RED)

The headline failure mode and the one the BAIR blog flags as TinyAgent's main
risk. The model invents plausible-sounding tools that aren't in the surface:

- **Case 16** ("Find a cooperative datapod about weaving and save it to my index"):
  ```
  1. search_local_index("weaving", 10)
  2. create_datapod("weaving", "cooperative", "", 10)
  3. join()
  ```
  Both tools are wrong: `search_local_index` should be `search_subnet`, and
  `create_datapod` is fabricated (the actual tool is `crawl_index`).
- **Case 21** ("Email my wife that I'll be late"):
  ```
  1. compose_email("I'll be late", "I'll be late", "", [])
  2. join()
  ```
  `compose_email` is fabricated. The model was trained on `compose_new_email`
  for the BAIR Apple-tools surface and is reaching back to its training
  distribution because no Lupus tool plausibly fits.

The shape of `compose_email` matches the upstream `compose_new_email`
signature exactly (5 positional args), confirming this is a training-data
echo, not an LLMCompiler grammar error.

### 2. Tool confusion / mis-selection (4/22)

The model picks the wrong tool from the available 6:

- **Case 7** ("Find datapods about open-source 3D printing"): picks
  `search_local_index` instead of `search_subnet`. The word "datapods" should
  be a strong subnet signal but the model defaults to local search.
- **Case 11** ("Is https://paypa1-secure.support/login.php safe?"): picks
  `search_subnet("...", "hvym")` instead of `scan_security`. The model
  doesn't connect "is X safe?" to the security tool; it treats "hvym" as a
  free-floating keyword.
- **Case 15** ("Fetch X and tell me if it's dangerous"): only emits
  `fetch_page`, drops the `scan_security` follow-up. The model executes the
  first verb in the sentence and forgets the second clause.
- **Case 17** had the same one-clause-only failure but the parser had been
  swallowing step 1; with the parser fix it now passes.

### 3. Over-helpful expansion (2/22)

- **Cases 9, 10** ("Fetch <URL>"): the model emits both `fetch_page` and
  `extract_content("$1")`. Arguably defensible — the user said "fetch", which
  could mean "get the content", and the model proactively extracts it. But
  it's not what was asked, and it inflates the chain length.

  This is a gray-area failure. Whether it's a model bug or a too-narrow
  expected-set is a judgment call. Leaving as a real failure for now because
  the daemon should not be making unsolicited tool calls.

### 4. Excess caution / abstention (2/22)

- **Cases 12, 13**: the model emits a bare `join()` instead of calling the
  obviously-required tool.
  - Case 12 ("Check https://github.com/inviti8/lupus for threats") needs
    `scan_security` but the model abstains. Hypothesis: `scan_security`
    requires `(html, url)` and the model can't satisfy `html` without
    `fetch_page` first, and gives up rather than chaining.
  - Case 13 ("Add https://... to my index") needs `crawl_index`. The model
    abstains. Probably the wording "add ... to my index" doesn't match the
    `crawl_index` tool name strongly enough.

### 5. Argument arity (4/22, soft fails)

Cases 9, 10, 14, 18 all have `extract_content("$1")` — model drops the
required `format` argument. **I tried tightening the description** between
runs (added "You MUST always pass it; default to 'full'"); the model still
dropped it. The arity check correctly flags these as soft fails.

This is a real model weakness, not a prompt issue. The TinyAgent custom
instructions already say "You MUST fill every argument" and the model just
doesn't comply. Likely needs an in-context example showing the
2-argument form.

---

## Phase 6 decision tree walk

Branches are evaluated **in order**; first match wins (per `docs/TINYAGENT_EVAL.md` lines 274-287).

| # | Branch | Trigger | Fires? |
|---|---|---|---|
| 1 | Syntactic validity < 70% | 100% — ✗ | no |
| 2 | All green | selection RED, halluc RED — ✗ | no |
| 3 | Tool selection 60-80% otherwise green | 59.1% **0.9pp below 60%** — ✗ technically | no |
| 4 | Halluc > 5% OR abstention < 50% | 9.1% > 5% — ✓ | **YES** |
| 5 | Multi-step < 40% but selection ≥ 80% | doesn't apply | no |
| 6 | Selection < 60% AND arg shape < 75% | arg shape 81.8% — ✗ | no |
| 7 | JSON marker format was the only gap | partial — selection still RED | no |

**Strict reading: branch 4 fires.** Recommended action per the tree:

> First try ToolRAG: pull `squeeze-ai-lab/TinyAgent-ToolRAG` to filter the
> tool list per query; this is what BAIR trained for. If still bad, this is
> a small-LoRA case: fine-tune on `knowledge_aware.jsonl` plus 100-200
> hand-written negative examples (adversarial queries with `join_finish()`
> targets).

---

## My recommendation: try a cheaper intervention before ToolRAG

The decision tree's branch-4 prescription assumes ToolRAG is the cheapest
non-training intervention. After looking at the actual failures, **I think
two cheaper interventions are worth trying first**, in this order:

### Step A — Add 3-5 targeted in-context examples (~30 min, $0)

The model already has 3 examples in the planner prompt. Adding 3-5 more
that target the specific failure modes would directly address 6 / 11
failures with no model change:

| Add example showing… | Fixes case |
|---|---|
| `search_subnet` for a "datapods about X" query | 7 |
| `scan_security` for an "is X safe?" query (with empty html) | 11, 12 |
| `crawl_index` for "add ... to my index" | 13 |
| `fetch_page → scan_security` chain | 15 |
| Two-arg `extract_content("$1", "full")` | 9, 10, 14, 18 (arg shape) |

This is essentially in-context learning. Each example is two lines. We're
0.9 percentage points below the branch-3 yellow threshold; a few targeted
examples will almost certainly cross it.

### Step B — If Step A leaves halluc > 5%, try ToolRAG (~2 hours, $0-$2)

The 2 actual hallucinations (`create_datapod`, `compose_email`) are exactly
what ToolRAG is built for: pre-filter the tool list by query so the model
only sees tools it could plausibly need. ToolRAG is a separately-trained
embedding model; pulling it costs nothing, plumbing it in is ~50 lines.

If both cases 16 and 21 stop hallucinating after ToolRAG, we're at
0% hallucination and we can re-evaluate the rest.

### Step C — Only if A+B leaves selection still < 60%: small targeted LoRA

Per branch 6 of the decision tree. At that point we'd have evidence that
prompt engineering can't carry the model and we need adapter weights. The
existing LoRA infra (`base/config.yaml`, `training/train_security.py` scaffold)
is reusable.

### Why not jump to "ship as-is"

Tool selection at 59.1% means **40% of user queries get the wrong tool**.
That's not shippable as the daemon's planner. The daemon would call wrong
tools, return bogus results to the browser, and erode user trust fast. The
hallucination rate (9.1%) is the one that scares me most: the daemon would
literally try to dispatch `compose_email` and crash.

### Why not skip directly to LoRA training

The Phase 5 deliverable is "evidence-based decision". The evidence says the
model already knows the LLMCompiler grammar perfectly (100% syntactic
validity, working `$N` chains, working abstention) — what it doesn't have
is **familiarity with our specific tool surface**. That's exactly the
problem in-context examples + ToolRAG were designed to solve. Training a
LoRA when the cheaper intervention hasn't been tried yet is premature
optimization.

---

## Concrete next actions

In order, no parallelism needed:

1. **Step A (in-context examples)** — extend `LUPUS_IN_CONTEXT_EXAMPLES` in
   `tools/tinyagent_prompt_probe.py` with the 5 examples listed above. Re-run
   `python tools/eval_tinyagent.py`. Goal: tool selection ≥ 80%, hallucination
   ≤ 5%, arg shape ≥ 90%. Expect this to take 20-40 minutes.
2. **If Step A clears all GREEN → ship-as-is path**: bake the example list
   into `daemon/src/tools/mod.rs::system_prompt()` (the deferred work item
   #28) and integrate the parser. Drop training entirely.
3. **If Step A clears yellow but halluc still > 5% → Step B** (ToolRAG).
4. **If Step B still leaves selection < 60% → Step C** (small LoRA on
   ~200 hand-written negative examples + the existing
   `knowledge_aware.jsonl` rewritten to LLMCompiler format).

Each step decides whether to take the next one. Don't pre-commit to Step C.

---

## Items the eval surfaced that aren't model failures

For honesty: a few cases are arguably ground-truth issues, not model bugs.

- **Cases 9, 10**: "Fetch X" reasonably means "get the content of X", and
  `fetch_page` returns raw bytes. A user expecting readable text would
  reasonably want `extract_content` too. I left these as failures because
  the daemon should not be making unsolicited tool calls — but if Step A
  doesn't fix them, consider whether to broaden the expected set.
- **Case 11 (alt accepted)**: I added `acceptable_alt_tools=[{fetch_page,
  scan_security}]` for case 11 but the model picked `search_subnet`, which
  is neither — it's plain wrong. The acceptable-alt fallback didn't save it.

---

## Operational notes

- `llama-cpp-python` 0.3.20 built from source on Windows in ~7 minutes via
  the prebuilt wheel index (it fell back to a local build because the index
  didn't have a Windows binary for our Python). Once built it's stable. CPU
  inference is ~2 seconds per case for these short outputs.
- Determinism: temperature 0 + the same system prompt produces the same
  output across runs. Changing the system prompt (e.g. tightening the
  `extract_content` description) shifted some outputs as a side-effect of
  changing the input token sequence — this is expected.
- The eval JSONL is gitignored (`*.log` glob, but `*.jsonl` slips through —
  but `eval/tinyagent_runs/` parent dir is tracked via `.gitkeep`). Future
  runs will append `<timestamp>-eval.jsonl` files.
