# TinyAgent Step A — In-Context Examples Findings

**Date**: 2026-04-07
**Final eval run**: `eval/tinyagent_runs/20260407-225450-eval.jsonl`
**Iteration count**: 6 example-set variations tested
**Total wall time**: ~10 min (each run ~50 s; iteration on the prompt itself)
**Cost**: $0

---

## Result: YELLOW plateau, escalating to Step B (ToolRAG)

Step A moved the model from RED to YELLOW on tool selection and hallucination,
but **prompt engineering on a 1.1B model has a clear ceiling**. Six iterations
of in-context examples all converged on the same hard-pass count (13/22) and
roughly the same tool selection (72-73%). Each example change traded one
failure for another.

| Metric | Pre-Step A | **Step A final** | Δ |
|---|---|---|---|
| Syntactic validity | 100.0% 🟢 | 100.0% 🟢 | — |
| Tool selection accuracy | 59.1% 🔴 | **72.7% 🟡** | +13.6 pp |
| Argument shape validity | 81.8% 🟡 | **86.4% 🟡** | +4.6 pp |
| Hallucinated tool rate | 9.1% 🔴 | **4.5% 🟡** | -4.6 pp |
| Multi-step correctness | 60.0% 🟢 | **80.0% 🟢** | +20 pp |
| Abstention correctness | 75.0% 🟢 | **100.0% 🟢** | +25 pp |
| **Hard pass** | 11/22 | **13/22** | +2 |

The eval no longer hard-fails. We crossed every YELLOW threshold and pushed
multi-step + abstention into GREEN. Tool selection and hallucination stalled
in YELLOW.

---

## What worked

The **7-example final set** (`tools/tinyagent_prompt_probe.py:140-194`)
covers each of the 6 Lupus tools at least once and includes one abstention
example for off-surface queries. The full list:

| # | Pattern | Wording cue | Fixes |
|---|---|---|---|
| 1 | `search_local_index` alone | "Find pages in my local index about X" | baseline |
| 2 | `search_subnet` alone | "Find datapods about X" | (none — case 7 still fails) |
| 3 | `crawl_index` alone | "Add X to my index" | (none — case 13 inconsistent) |
| 4 | `fetch_page → scan_security` chain | "Is X safe?" | case 11 partially |
| 5 | `search_subnet → crawl_index` chain | "Find a datapod about X and save it" | case 16 |
| 6 | `search_local_index → fetch_page` chain | `$1` reference demo | case 17 |
| 7 | bare `join()` abstention | "Translate French→German" off-surface | case 21 |

Cases that flipped from X to ok with this set: **16** (search_subnet → crawl_index
chain, was hallucinating `create_datapod`), **17** ($N reference, was a parser
bug), **21** (was hallucinating `compose_email`).

---

## What didn't work — the whack-a-mole pattern

Six iterations are documented in `eval/tinyagent_runs/`:

| Run | Examples | Tool sel | Halluc | Abstn | Hard pass | Notes |
|---|---|---|---|---|---|---|
| 1 | 3 (baseline) | 59.1% 🔴 | 9.1% 🔴 | 75% 🟢 | 11/22 | initial |
| 2 | 3 (post parser/desc fix) | 59.1% 🔴 | 9.1% 🔴 | 75% 🟢 | 11/22 | parser anchor + extract_content desc |
| 3 | 9 (full set) | 59.1% 🔴 | **0.0% 🟢** | **100% 🟢** | 11/22 | hyperfit on `fetch+extract` template |
| 4 | 7 (trimmed) | **72.7% 🟡** | 4.5% 🟡 | **100% 🟢** | **13/22** | best halluc + abstention |
| 5 | 9 (run 4 + saved + fetch alone) | 72.7% 🟡 | 9.1% 🔴 | 50% 🟡 | 13/22 | over-abstain regressions |
| 6 | 8 (run 4 + saved variant only) | 72.7% 🟡 | 9.1% 🔴 | 75% 🟢 | 13/22 | similar tradeoff |

**Three observations**:

1. **The 1.1B model has a context-noise ceiling.** Adding more examples
   doesn't monotonically improve performance — past ~7 examples the model
   starts pattern-matching too aggressively (case 1 in run 3 emitted both
   `search_local_index` AND `search_subnet` because both example wordings
   matched a wolves-in-local-index query) or abstaining when uncertain
   (cases 2, 5 in run 3 abstained because no example wording matched
   "What did I save").

2. **The `fetch_page → extract_content` chain example was actively harmful.**
   The model treated it as a universal "URL processing" template and
   repurposed the `format` slot as a free-form keyword:
   - `extract_content("$1", "status")` for "Is X safe?"
   - `extract_content("$1", "summary")` for "Add to my index"
   - `extract_content("$1", "dangerous")` for "is it dangerous"

   Removing it (run 3 → run 4) was the single biggest improvement — selection
   jumped from 59.1% to 72.7%. The trade-off: cases 14 and 18 fall to soft
   fails because they use 1-arg `extract_content("$1")`, but the cases get
   the right tools.

3. **The remaining hallucinations are query-driven, not prompt-driven.**
   Case 9 invents `summarize` because the user query is "Fetch <URL>" with
   no "and X" continuation, and the model auto-completes user intent with a
   plausible-sounding tool name. Case 21 invents `send_email` because the
   user explicitly asks to send email and the only abstention example is
   off-topic (translation). No amount of prompt iteration fixed these
   without breaking other cases.

---

## What's left after Step A

11 cases pass cleanly (🟢). 9 cases fail in 4 different ways:

| Failure mode | Cases | Root cause | Step A could fix? |
|---|---|---|---|
| Tool confusion | 7, 12, 15 | Wording cue too weak ("datapods" loses to "find", "check for threats" loses to abstain, "tell me if dangerous" gets dropped) | partially — see 9-example variants |
| Over-helpful expansion | 9, 10 | Model auto-completes "Fetch X" → "Fetch X and process" | no — query-driven |
| Hallucinations | 9 (`summarize`), 21 (`send_email`) | No matching tool, model invents one rather than abstain | no — needs tool restriction |
| Soft fails (arg shape) | 11, 14, 18 | `extract_content("$1")` 1-arg form, `scan_security` arity | borderline — adding 2-arg examples didn't help |

The first and third categories are exactly what **Step B (ToolRAG)** is
designed to fix. ToolRAG is a separately-trained embedding model from BAIR
(`squeeze-ai-lab/TinyAgent-ToolRAG`) that filters the visible tool list per
query before the planner sees it. If the planner only sees the 1-2 most
relevant tools for "Email my wife", `send_email` wouldn't be invented because
the planner couldn't reach for it; the planner would be forced to abstain or
pick from the available tools.

ToolRAG also addresses the wording-cue weakness by doing semantic matching
on the user query against tool descriptions, which is exactly what
in-context examples were trying (and failing) to do via pattern-matching.

---

## Comparison with Phase 5 thresholds

| Metric | Step A final | Threshold | Color |
|---|---|---|---|
| Syntactic validity | 100.0% | ≥ 90% / ≥ 70% | 🟢 GREEN |
| Tool selection | **72.7%** | ≥ 80% / ≥ 60% | 🟡 YELLOW |
| Arg shape | 86.4% | ≥ 90% / ≥ 75% | 🟡 YELLOW |
| Hallucination | 4.5% | ≤ 2% / ≤ 5% | 🟡 YELLOW |
| Multi-step | 80.0% | ≥ 60% / ≥ 40% | 🟢 GREEN |
| Abstention | 100.0% | ≥ 75% / ≥ 50% | 🟢 GREEN |

We sit in the "all yellow or better" band. **Tool selection is 7.3 percentage
points below the green threshold and hallucination is 2.5 percentage points
above it.** Both gaps are closeable but neither closes via prompt engineering
alone — we tried.

---

## Phase 6 decision tree, revisited

| # | Branch | Condition | Verdict |
|---|---|---|---|
| 1 | Validity < 70% | 100% — ✗ | no |
| 2 | All green | selection YELLOW, halluc YELLOW, args YELLOW — ✗ | no |
| 3 | Selection 60-80% otherwise green | 72.7% in range; "otherwise green"? args YELLOW, halluc YELLOW — partial ✓ | **borderline yes**, but we already iterated on examples 6 times |
| 4 | Halluc > 5% OR abstention < 50% | halluc 4.5% (just under), abstention 100% — ✗ | no |
| 5 | Multi-step < 40% but selection ≥ 80% | doesn't apply | no |
| 6 | Selection < 60% AND args < 75% | selection 72.7%, args 86.4% — ✗ | no |
| 7 | JSON marker only | partial | partial |

**Reading**: branch 3 was the right branch coming out of Phase 5 and we've
exhausted what it can give us. Halluc is fractionally below the 5% YELLOW
threshold (so branch 4 doesn't strictly fire) but the failure-mode analysis
above shows the remaining hallucinations are exactly what ToolRAG was built
for. **The cheapest meaningful intervention left is ToolRAG (Step B).**

---

## Step B preview (next session)

ToolRAG is a separate inference call that runs *before* the planner:

```
user query → [ToolRAG embedding search] → top-k relevant tools
                       ↓
   [planner with filtered tool list] → plan → [executor] → [joinner] → answer
```

Mechanics from `dist/tinyagent-source/src/tiny_agent/tool_rag/`:
- ToolRAG is a small Sentence-Transformers model (~80 MB) fine-tuned by BAIR
- Input: user query + full tool list with descriptions
- Output: top-K (typically 3) tool names ranked by relevance
- Plumbing: substitute the filtered list into the planner's `{tool_descriptions}` slot

Cost: ~2 hours dev + ~0.1 s extra per query at inference time. Zero cloud
spend if we run locally. The ToolRAG model is small enough to load alongside
the planner GGUF on CPU.

**Expected impact**: case 9 (`summarize`) would be filtered out because the
embedding match for "Fetch <URL>" wouldn't include any extraction tool;
case 21 (`send_email`) would have no email tool in the filtered list at all,
forcing the planner to use the `join()` abstention path it now knows; cases
12, 13, 15 would be more likely to pick the right tool because the planner
only sees 2-3 candidates instead of 6.

If ToolRAG closes the gap to GREEN, we ship. If it doesn't, we go to Step C
(small targeted LoRA on ~200 hand-written planner training examples).

---

## Operational notes

- The 6 eval iterations are all preserved in `eval/tinyagent_runs/`. Each
  is a JSONL file with the case, raw output, parsed plan, scores, and
  per-case elapsed time. Diffing two runs is just `diff <(jq -c '...' run1)
  <(jq -c '...' run2)`.
- The probe script and the eval script share `LUPUS_IN_CONTEXT_EXAMPLES` and
  `build_planner_system_prompt` via direct import — there's exactly one
  source of truth for the rendered prompt. Future iterations only need to
  edit `tools/tinyagent_prompt_probe.py`.
- Inference is now ~50 s per 22-case eval run on CPU (~2.3 s per case),
  consistent across the 6 runs. The CPU build is stable.
- I did not retrain or download any model during Step A. All variation was
  in the system prompt.
