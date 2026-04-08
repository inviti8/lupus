#!/usr/bin/env python3
"""Lupus ToolRAG — query-to-tool cosine retrieval over our 6-tool surface.

Step B of docs/TINYAGENT_EVAL.md / docs/TINYAGENT_STEPA_FINDINGS.md.

Why not use BAIR's `squeeze-ai-lab/TinyAgent-ToolRAG`?
    BAIR's classifier is a 16-class model hardwired to their Apple-app tool
    surface (`compose_new_email`, `send_sms`, etc.) — see
    `dist/tinyagent-source/src/tiny_agent/tool_rag/classifier_tool_rag.py:23-40`.
    Its outputs cannot map to any Lupus tool. Useless for us.

What we use instead:
    Plain Sentence-Transformers cosine similarity over the 6 Lupus tool
    descriptions. We have so few tools that a classifier is overkill;
    direct embedding similarity is plenty. This mirrors `SimpleToolRAG`
    from upstream but without the pickle of curated examples — we go
    straight from query embedding to tool embedding.

Architecture:
    1. Pre-compute one embedding per Lupus tool (description text)
    2. At query time: embed the user query
    3. Cosine similarity between query and each tool
    4. Return the top-K tool names (default K=3)

The eval script substitutes the filtered tool list into the planner's
system prompt for that one query. The planner sees fewer distractors
and is less likely to hallucinate or confuse tools.

Cost: ~5-10 ms per query on CPU after a one-time model load (~80 MB).
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tools"))

from tinyagent_prompt_probe import LUPUS_TOOLS, LupusTool  # noqa: E402

# Sentence-Transformers embedding model.
#   - all-MiniLM-L6-v2 (22 MB, 384-dim): the default. Too lexical for our
#     6-tool surface — scored "Show me saved articles" closer to
#     extract_content than search_local_index because of "articles" word
#     overlap. See docs/TINYAGENT_STEPB_FINDINGS.md.
#   - all-mpnet-base-v2 (420 MB, 768-dim): a larger, more semantic alternative.
#     Not validated; download stalled at 64 MB during the Step B experiment.
EMBEDDING_MODEL_NAME = "sentence-transformers/all-MiniLM-L6-v2"


class LupusToolRAG:
    """Cosine-similarity tool retrieval over the Lupus tool surface."""

    def __init__(self, tools: list[LupusTool] = LUPUS_TOOLS) -> None:
        os.environ.setdefault("TRANSFORMERS_VERBOSITY", "error")
        # Local import: keeps the module cheap to import without ST installed.
        from sentence_transformers import SentenceTransformer

        self._tools = tools
        self._model = SentenceTransformer(EMBEDDING_MODEL_NAME)
        # Pre-compute one embedding per tool, normalized for cosine sim.
        self._tool_embeddings = self._model.encode(
            [tool.description for tool in tools],
            normalize_embeddings=True,
            convert_to_numpy=True,
        )

    def retrieve(
        self,
        query: str,
        top_k: int = 3,
        primary_threshold: float = 0.10,
        secondary_threshold: float = 0.04,
    ) -> list[LupusTool]:
        """Return up to top_k most relevant Lupus tools for the given query.

        Dual-threshold semantics:
        - If the *top* tool's score is below `primary_threshold`, return [].
          This is the abstention signal: nothing matches well enough.
        - Otherwise, return all top_k tools whose score >= `secondary_threshold`.
          This catches the secondary tools needed for chained queries
          (e.g. "Is X safe?" needs both scan_security and fetch_page,
          where fetch_page scores ~0.05 but is essential to the chain).

        Thresholds tuned empirically from `lupus_tool_rag.py main()`:
        - In-distribution top scores: 0.20-0.50
        - In-distribution secondary scores: 0.04-0.40
        - Out-of-distribution (translation, math, email): all < 0.10 or negative
        """
        query_embedding = self._model.encode(
            query,
            normalize_embeddings=True,
            convert_to_numpy=True,
        )
        # Cosine similarity == dot product when both sides are L2-normalized.
        similarities = self._tool_embeddings @ query_embedding
        # Indices of the top_k highest similarities, in descending order.
        top_indices = similarities.argsort()[::-1][:top_k]
        # Abstention signal: top match too weak.
        if similarities[top_indices[0]] < primary_threshold:
            return []
        return [
            self._tools[i]
            for i in top_indices
            if similarities[i] >= secondary_threshold
        ]

    def retrieve_with_scores(
        self, query: str, top_k: int = 3
    ) -> list[tuple[LupusTool, float]]:
        """Same as retrieve() but also returns the cosine similarities,
        useful for debugging the retrieval quality."""
        query_embedding = self._model.encode(
            query,
            normalize_embeddings=True,
            convert_to_numpy=True,
        )
        similarities = self._tool_embeddings @ query_embedding
        top_indices = similarities.argsort()[::-1][:top_k]
        return [(self._tools[i], float(similarities[i])) for i in top_indices]


def main() -> int:
    """Smoke test: print top-3 retrievals for a handful of queries so the
    operator can sanity-check the embedding match before plumbing it into
    the eval."""
    print(f"Loading {EMBEDDING_MODEL_NAME}...")
    rag = LupusToolRAG()
    print(f"  loaded. {len(rag._tools)} tools indexed.\n")

    test_queries = [
        ("Find pages about wolves in my local index", "search_local_index"),
        ("What did I save about Anishinaabe folklore?", "search_local_index"),
        ("Search my local history for rust borrow checker explanations", "search_local_index"),
        ("Any pages mentioning IPFS content routing?", "search_local_index"),
        ("Show me saved articles about wool felting", "search_local_index"),
        ("Find datapods about open-source 3D printing", "search_subnet"),
        ("Fetch https://example.com/article.html", "fetch_page"),
        ("Is https://paypa1-secure.support/login.php safe?", "scan_security"),
        ("Check https://github.com/inviti8/lupus for threats", "scan_security"),
        ("Add https://wikipedia.org/wiki/Wolf to my index", "crawl_index"),
        ("Email my wife that I'll be late", "(abstain)"),
        ("What is 2+2?", "(abstain)"),
    ]

    for q, expected in test_queries:
        # Use top_k=6 to see the FULL ranking including the expected tool
        results = rag.retrieve_with_scores(q, top_k=6)
        marker = "ok" if results and results[0][0].name == expected else "X"
        print(f"[{marker}] Q: {q}")
        print(f"     expected: {expected}")
        for tool, score in results:
            star = "*" if tool.name == expected else " "
            print(f"   {star} {score:.3f}  {tool.name}")
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
