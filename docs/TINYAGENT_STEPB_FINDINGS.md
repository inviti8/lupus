# TinyAgent Step B — ToolRAG Findings (Negative Result)

**Date**: 2026-04-07 / 2026-04-08
**Status**: ToolRAG abandoned for our 6-tool surface; `USE_TOOL_RAG = False` in `tools/eval_tinyagent.py`
**Best ToolRAG metrics**: 10/22 hard pass (vs Step A baseline 13/22)
**Recommendation**: skip to Step C (planner LoRA)

---

## TL;DR

ToolRAG with off-the-shelf Sentence-Transformers embeddings is **strictly worse** than no ToolRAG on our 6-tool surface. The cure is more expensive than the disease. We tested:

- `all-MiniLM-L6-v2` (22 MB, 384-dim) — too lexical, can't disambiguate semantically similar words
- Started downloading `all-mpnet-base-v2` (420 MB, 768-dim) — bailed mid-experiment after structural diagnosis showed the issue is architectural, not just embedding-quality

The Step B infrastructure (`tools/lupus_tool_rag.py`, the per-query tool filtering in `tools/eval_tinyagent.py`, the in-context example filtering in `tools/tinyagent_prompt_probe.py::build_in_context_examples`) is preserved for future revisit but disabled. Step A's 7-example prompt remains the production baseline.

---

## What ToolRAG is supposed to do

Per the BAIR paper and `dist/tinyagent-source/src/tiny_agent/tool_rag/`: ToolRAG is a **pre-planner filter**. Before the planner sees the tool list, ToolRAG embeds the user query, computes cosine similarity against each tool description, and returns the top-K most relevant tools. The planner then sees a *filtered* tool surface — typically 2-3 tools instead of 16 — which:

1. Reduces context noise (fewer distractors → less confusion)
2. Bounds the model's choice (less room for hallucination)
3. Compresses the prompt (token savings)

BAIR's production ToolRAG is a **fine-tuned classifier** (`squeeze-ai-lab/TinyAgent-ToolRAG`), 16-class output, hardwired to their Apple-app tool surface. We can't use it directly — the labels are baked in to BAIR's tool names, not ours (`classifier_tool_rag.py:23-40`).

What we *can* lift is the simpler `SimpleToolRAG` design (`simple_tool_rag.py`): generic Sentence-Transformers cosine similarity over tool descriptions, no fine-tuning needed.

---

## What we built (`tools/lupus_tool_rag.py`)

A 100-line `LupusToolRAG` class:

```python
class LupusToolRAG:
    def __init__(self, tools=LUPUS_TOOLS):
        self._model = SentenceTransformer("sentence-transformers/all-MiniLM-L6-v2")
        self._tool_embeddings = self._model.encode(
            [tool.description for tool in tools],
            normalize_embeddings=True,
        )

    def retrieve(self, query, top_k=3,
                 primary_threshold=0.10, secondary_threshold=0.04):
        query_emb = self._model.encode(query, normalize_embeddings=True)
        sims = self._tool_embeddings @ query_emb
        top = sims.argsort()[::-1][:top_k]
        if sims[top[0]] < primary_threshold:
            return []   # abstention signal
        return [self._tools[i] for i in top if sims[i] >= secondary_threshold]
```

Plus the dual-threshold filtering, the `build_in_context_examples` example-filter (so the planner doesn't see chain examples calling tools that have been filtered out), and integration into the eval script's per-query system prompt rebuild.

---

## Iterations and metrics

| Run | Setup | Tool sel | Halluc | Multi-step | Abstn | Hard pass |
|---|---|---|---|---|---|---|
| Step A baseline | No RAG, 7 examples | **72.7% 🟡** | **4.5% 🟡** | 80% 🟢 | **100% 🟢** | **13/22** |
| Step B run 1 | RAG on, single threshold 0.10 | 36.4% 🔴 | 4.5% 🟡 | **0% 🔴** | 100% 🟢 | 8/22 |
| Step B run 2 | RAG on, dual threshold (0.10/0.04) + filtered abstention example + restored fetch+extract chain | 54.5% 🔴 | **0% 🟢** | 40% 🟡 | 100% 🟢 | 10/22 |

ToolRAG flipped two metrics in opposite directions: **hallucination went to GREEN** (model could no longer reach for filtered-out fabricated tools) and **abstention stayed at GREEN** (the empty-rag-list case correctly forced abstention). But **tool selection collapsed** because the embedding model was wrong about which tools to surface, and **multi-step collapsed** because chain queries got incomplete tool lists.

Net: a 5-case regression vs Step A.

---

## Root cause: the embedding model is doing lexical word overlap, not semantic intent matching

The full ranking from `tools/lupus_tool_rag.py` smoke test on the failing queries (`all-MiniLM-L6-v2`):

```
Q: Search my local history for rust borrow checker explanations
   expected: search_local_index
     0.141  scan_security        ← "Search" matches "Scan"
     0.115  crawl_index
     0.115  search_subnet        ← "Search" matches "Search"
   * 0.090  search_local_index   ← actual answer, ranked 4th
     0.044  fetch_page
     0.041  extract_content

Q: Any pages mentioning IPFS content routing?
   expected: search_local_index
     0.460  crawl_index          ← "content" matches "Fetch content by CID"
     0.239  fetch_page
     0.179  search_subnet
     0.142  scan_security
   * 0.109  search_local_index   ← actual answer, ranked 5th
     0.068  extract_content

Q: Show me saved articles about wool felting
   expected: search_local_index
     0.174  extract_content      ← "articles" matches "Extract clean text"
     0.153  crawl_index
     0.140  scan_security
   * 0.085  search_local_index   ← actual answer, ranked 4th
     0.058  fetch_page
    -0.030  search_subnet
```

The pattern is unambiguous. The embedding model is matching on:
- **"Search"** in the query → **`scan_security`** ("Scan...") and **`search_subnet`** ("Search...")
- **"content"** in the query → **`crawl_index`** ("Fetch content by CID...")
- **"articles"** in the query → **`extract_content`** ("Extract clean text...")

These are surface-level word collisions, not semantic understanding. The right tool (`search_local_index` for queries about saved/history/pages) consistently ranks 4th-5th out of 6.

The 22 MB MiniLM model wasn't trained to disambiguate "I want to find pages I previously saved" from "I want to extract text". It's trained for general-purpose sentence similarity, where lexical overlap dominates.

---

## Why bigger embedding models probably won't fix this either

We started downloading `all-mpnet-base-v2` (420 MB, 768-dim) — the standard "best small" Sentence-Transformers model. The HF Hub download stalled at 64 MB and we bailed when the structural analysis below made the experiment unappealing regardless of outcome.

Even if mpnet ranked correctly on cases 3, 4, 5, **ToolRAG has a structural problem on a 6-tool surface that no embedding model can fix**:

### Structural problem 1: 50% removal rate is too aggressive

With `top_k=3` of 6 tools, ToolRAG removes 3 tools per query. **That's 50% of the surface gone every time.** Compare to BAIR's setup (top-3 of 16) which removes ~80% but starts with much more redundancy.

In a 6-tool surface, every tool is doing distinct work. Removing the wrong half is unrecoverable — the planner can't pick a tool it can't see, regardless of how many in-context examples exist.

### Structural problem 2: chain examples become orphans

When ToolRAG returns a single tool (e.g. `{scan_security}`), the in-context example for chain calls `{fetch_page, scan_security}` gets filtered out (it's not a subset of `{scan_security}`). The planner sees the tool listed but **no demonstration of how to call it**, and falls back to abstention. This is exactly what happened in run 1 cases 11 and 12.

We patched this in run 2 with a dual threshold (always include marginal tools in top-k), but the patch is fragile: it works only when the secondary tool happens to score above 0.04.

### Structural problem 3: cascading mistakes

In a multi-stage system, the upstream stage's mistakes are the downstream stage's hard constraints. ToolRAG's wrong filter is the planner's only world. **No prompt engineering or even fine-tuning of the *planner* can recover from a missing tool in the input** — the planner can't pick what it can't see. The only fix is to make the upstream stage perfect, which for ours means either (a) a much better embedding model that won't cascade mistakes, or (b) a fine-tuned tool classifier specifically trained on Lupus queries.

Option (b) is essentially "train a small model so we don't have to train a small model" — at which point we should just train the planner LoRA directly and skip the indirection.

### Structural problem 4: ToolRAG was designed for many-tool surfaces

BAIR built ToolRAG for their 16-tool surface where context noise is a real bottleneck and where the embedding model has plenty of distinct tools to choose between. **For 6 tools, the planner can hold all of them in working memory**, and our Step A eval shows it does — selection plateaus at 72.7% with full context, not because of context noise but because the model's prior is wrong on certain wordings.

The right fix for that is not a smaller candidate set; it's a **shifted prior**, which is exactly what fine-tuning does.

---

## What we kept for future revisit

The Step B infrastructure is preserved but disabled:

| File | Status | Purpose |
|---|---|---|
| `tools/lupus_tool_rag.py` | kept, working | Standalone smoke test still runs; reusable if we ever scale to 15-20 tools |
| `tools/eval_tinyagent.py` | `USE_TOOL_RAG = False` | One-line flip to re-enable for experiments |
| `tools/tinyagent_prompt_probe.py::build_in_context_examples` | refactored | Now takes an `include_abstention` flag; cleanly handles both narrowed-RAG and full-surface modes |
| `tools/tinyagent_prompt_probe.py::LUPUS_EXAMPLES` | refactored | Each example tagged with `tools_used` so future RAG modes can filter |
| `dist/lupus-tool-rag/` (sentence-transformers cache) | downloaded | The 22 MB MiniLM model is in the HF Hub cache; no cleanup needed |

The 420 MB mpnet-base-v2 model was **not** fully downloaded — about 64 MB sit in the HF Hub cache. They can be cleaned up or left alone (cache eviction will handle them eventually).

---

## The decision-tree branch we landed on

Per `docs/TINYAGENT_EVAL.md` Phase 6 decision tree:

> **Branch 4**: Halluc > 5% OR abstention < 50% → first try ToolRAG. If still bad, this is a small-LoRA case: fine-tune on `knowledge_aware.jsonl` plus 100-200 hand-written negative examples.

We tried ToolRAG. It was strictly worse. The "if still bad" condition fires.

**Next stop: Step C — small targeted planner LoRA.**

---

## Why Step C should work where Step B didn't

The reason ToolRAG and prompt engineering both have ceilings on our surface is identical: **they work *around* the model's prior instead of changing it.** TinyAgent-1.1B has BAIR's Apple-app tool names baked into its weights. When the user says "Email my wife", the model's strongest token continuation is `compose_new_email(...)` because that's what it was trained to do. In-context examples (Step A) push back against that prior with demonstrations; ToolRAG (Step B) tries to deny the model access to its preferred token entirely. Both fail when the prior is strong enough.

Fine-tuning **changes the prior**. After a planner LoRA trained on Lupus queries:
- "Email my wife" produces `1. join()` because the model has *seen training examples* doing exactly this for email-shaped queries
- "Saved articles about wool felting" produces `1. search_local_index("wool felting", 10)` because the model has seen training examples doing exactly this for save-shaped queries
- The base model's LLMCompiler grammar capability is preserved by the LoRA's rank constraint (rank 16 only modifies a small subspace)

The expected jump (based on the security model v0.3 experience and BAIR's own LoRA numbers): tool selection 72% → 90-95%, hallucination 4.5% → 0-2%. We'd hit GREEN on every metric and enter the "ship as-is" branch of the decision tree.

---

## Step C plan summary (full plan in next session)

**Training data target**: 300-400 (query, LLMCompiler plan) pairs

| Source | ~Count | Effort |
|---|---|---|
| Multiply 22 eval cases × 8 wording variants | ~175 | 30 min hand-write |
| LLM-generated synthetic from Claude given the tool descriptions | 100-150 | 1 hour generate + curate |
| Adversarial / abstention (Email/SMS/Maps/Notes/Calendar bait) | 50-100 | 1 hour hand-write |
| Folklore data (`knowledge_aware.jsonl`) reformat to LLMCompiler | 30-40 | 30 min script |
| **Total** | **~350-450** | **~3 hours** |

**Hold out the original 22 eval cases** entirely from training so the eval measures generalization, not memorization.

**Training infrastructure** (mostly already exists from v0.3 security model):
- `tools/runpod_deploy.py` — auto-retry GPU deploy
- Network volume `b51y8tev3v` in EU-RO-1 (30 GB)
- LoRA config in `base/config.yaml` (rank 16, alpha 32, q/k/v/o_proj)
- New: `training/train_planner.py` — clone of `train_security.py` with `AutoModelForCausalLM` + `peft.LoraConfig`

**Cost / time**: 4-8 hours total work, $1-3 in GPU spend, single training run probably ~30 min on 4090.

**Risk to watch**: catastrophic forgetting of LLMCompiler grammar. Validate `eval_tinyagent.py`'s 100% syntactic validity metric on every checkpoint as a hard guardrail.
