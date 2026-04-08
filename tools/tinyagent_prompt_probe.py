#!/usr/bin/env python3
"""TinyAgent format probe — Phase 1 of docs/TINYAGENT_EVAL.md.

Goal: load `dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf`, render the 6 Lupus
tools into TinyAgent's native LLMCompiler prompt format, ask one trivial
question, and print the raw model output verbatim.

This is read-only — no scoring, no parsing, no daemon edits. The goal is
to confirm or refute the assumption that the daemon currently makes about
TinyAgent's output format (`<|function_call|>{json}<|end_function_call|>`).

Source-of-truth files copied verbatim from `dist/tinyagent-source/`:
  - planner system prompt scaffold from
      `src/llm_compiler/planner.py::generate_llm_compiler_prompt`
  - in-context examples from
      `src/tiny_agent/prompts.py::DEFAULT_PLANNER_IN_CONTEXT_EXAMPLES_PROMPT`
  - custom instructions from
      `src/tiny_agent/prompts.py::get_planner_custom_instructions_prompt`
  - join() description from `src/llm_compiler/planner.py::JOIN_DESCRIPTION`
  - END_OF_PLAN sentinel from `src/llm_compiler/constants.py`
"""

from __future__ import annotations

import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MODEL_GGUF = REPO_ROOT / "dist" / "tinyagent" / "TinyAgent-1.1B-Q4_K_M.gguf"

END_OF_PLAN = "<END_OF_PLAN>"


# ----------------------------------------------------------------------------
# Lupus tool surface — the 6 daemon tools, rendered in TinyAgent's native
# Python-signature-with-docstring format. This mirrors the rendering used
# by `dist/tinyagent-source/src/tiny_agent/tiny_agent_tools.py`, where each
# tool's `description` field is a multiline string of the form:
#
#     name(arg: type, ...) -> ret
#      - Bullet 1
#      - Bullet 2
#
# Source for each: `daemon/src/tools/<tool>.rs::schema()`.
# ----------------------------------------------------------------------------


@dataclass
class LupusTool:
    name: str
    description: str  # full multiline description, TinyAgent-native format


LUPUS_TOOLS: list[LupusTool] = [
    LupusTool(
        name="search_subnet",
        description=(
            "search_subnet(query: str, scope: str) -> dict\n"
            " - Search the cooperative subnet for matching datapod metadata.\n"
            " - 'query' is the search query string.\n"
            " - 'scope' is an optional subnet scope (e.g. 'hvym'); use an empty string if not applicable.\n"
            " - Returns a dict with a 'matches' list of datapod entries.\n"
        ),
    ),
    LupusTool(
        name="search_local_index",
        description=(
            "search_local_index(query: str, top_k: int) -> dict\n"
            " - Search the local semantic index for previously visited pages.\n"
            " - 'query' is the search query string.\n"
            " - 'top_k' is the maximum number of results; use 10 if not specified.\n"
            " - Returns a dict with a 'results' list of (url, title, summary, score) entries.\n"
        ),
    ),
    LupusTool(
        name="fetch_page",
        description=(
            "fetch_page(url: str) -> dict\n"
            " - Fetch page content by URL.\n"
            " - Supports both hvym:// datapod URLs and https:// URLs.\n"
            " - Returns a dict with 'url', 'content_type', 'body', and 'status'.\n"
        ),
    ),
    LupusTool(
        name="extract_content",
        description=(
            "extract_content(html: str, format: str) -> dict\n"
            " - Extract clean text, title, summary, and keywords from raw HTML.\n"
            " - 'html' is the raw HTML string to extract from. You MUST always pass it.\n"
            " - 'format' is either \"full\" or \"summary\". You MUST always pass it; default to \"full\" if the user did not specify.\n"
            " - Returns a dict with 'title', 'summary', 'content', 'keywords', and 'content_type'.\n"
            " - This tool can only be used AFTER calling fetch_page to get the HTML body.\n"
        ),
    ),
    LupusTool(
        name="scan_security",
        description=(
            "scan_security(html: str, url: str) -> dict\n"
            " - Scan an HTML page and its URL for security threats.\n"
            " - 'html' is the raw HTML body of the page.\n"
            " - 'url' is the URL of the page being scanned.\n"
            " - Returns a dict with a 'score' (0-100, higher is safer) and a 'threats' list.\n"
        ),
    ),
    LupusTool(
        name="crawl_index",
        description=(
            "crawl_index(source: str) -> dict\n"
            " - Fetch content by CID or URL and create a local index entry for it.\n"
            " - 'source' is either an IPFS CID or an https:// URL.\n"
            " - Returns a dict with 'indexed', 'url', and 'title'.\n"
        ),
    ),
]


# Verbatim from `src/llm_compiler/planner.py::JOIN_DESCRIPTION`
JOIN_DESCRIPTION = (
    "join():\n"
    " - Collects and combines results from prior actions.\n"
    " - A LLM agent is called upon invoking join to either finalize the user query or wait until the plans are executed.\n"
    " - join should always be the last action in the plan, and will be called in two scenarios:\n"
    "   (a) if the answer can be determined by gathering the outputs from tasks to generate the final response.\n"
    "   (b) if the answer cannot be determined in the planning phase before you execute the plans. "
)


# In-context examples, each tagged with the set of tools it uses. The
# `tools_used` set is consulted at prompt-build time to filter examples to
# only those that fit the currently-available tool surface — important for
# ToolRAG (Step B), which restricts the visible tool list per query. If
# we kept examples calling tools the planner has been told don't exist,
# the planner would learn conflicting signals.
#
# Step A iteration notes (final 7-example set):
#   - The fetch+extract chain example was REMOVED because the 1.1B model
#     was hyperfitting on it as a universal URL template, repurposing the
#     `format` slot as a free-form keyword (e.g. extract_content("$1", "dangerous")).
#   - The abstention example uses an off-surface task (translation) instead
#     of "send an email" so the model doesn't over-generalize to any
#     personal-sounding query.
#   - fetch_page alone was REMOVED to keep the example count low; cases 9/10
#     over-call extract_content but that's an acceptable gray-area failure.


@dataclass
class InContextExample:
    tools_used: frozenset[str]  # tool names referenced in the example body
    body: str  # the example text, ready to concatenate (with trailing ###\n)


LUPUS_EXAMPLES: list[InContextExample] = [
    # 1. single tool: search_local_index ("local index" wording)
    InContextExample(
        tools_used=frozenset({"search_local_index"}),
        body=(
            "Question: Find pages in my local index about wolves.\n"
            '1. search_local_index("wolves", 10)\n'
            "Thought: I have searched the local index.\n"
            f"2. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
    # 2. single tool: search_subnet (the "datapods" wording cue)
    InContextExample(
        tools_used=frozenset({"search_subnet"}),
        body=(
            "Question: Find datapods about decentralized art.\n"
            '1. search_subnet("decentralized art", "")\n'
            "Thought: I have searched the cooperative subnet for matching datapods.\n"
            f"2. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
    # 3. single tool: crawl_index (the "add to my index" wording cue)
    InContextExample(
        tools_used=frozenset({"crawl_index"}),
        body=(
            "Question: Add https://wikipedia.org/wiki/Wolf to my index.\n"
            '1. crawl_index("https://wikipedia.org/wiki/Wolf")\n'
            "Thought: I have indexed the page.\n"
            f"2. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
    # 4. multi-step chain: fetch_page -> scan_security ("is X safe?" cue)
    InContextExample(
        tools_used=frozenset({"fetch_page", "scan_security"}),
        body=(
            "Question: Is https://example.org/login.php safe?\n"
            '1. fetch_page("https://example.org/login.php")\n'
            '2. scan_security("$1", "https://example.org/login.php")\n'
            "Thought: I have fetched the page and scanned it for security threats.\n"
            f"3. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
    # 5. multi-step chain: search_subnet -> crawl_index ("find and save" pattern)
    InContextExample(
        tools_used=frozenset({"search_subnet", "crawl_index"}),
        body=(
            "Question: Find a datapod about felting and save it to my index.\n"
            '1. search_subnet("felting", "")\n'
            '2. crawl_index("$1")\n'
            "Thought: I have found the datapod and indexed it.\n"
            f"3. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
    # 6. multi-step chain: search_local_index -> fetch_page ($N reference)
    InContextExample(
        tools_used=frozenset({"search_local_index", "fetch_page"}),
        body=(
            "Question: Look up wolves in my local index, then fetch the first result in full.\n"
            '1. search_local_index("wolves", 10)\n'
            '2. fetch_page("$1")\n'
            "Thought: I have searched the local index and fetched the first result.\n"
            f"3. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
    # 7. abstention: off-surface task with no possible tool match (no tool refs)
    InContextExample(
        tools_used=frozenset(),  # empty: always included regardless of filter
        body=(
            "Question: Translate this French sentence to German: bonjour le monde.\n"
            "Thought: There is no tool available for language translation, so I cannot complete this request.\n"
            f"1. join(){END_OF_PLAN}\n"
            "###\n"
        ),
    ),
]


def build_in_context_examples(
    available_tool_names: set[str],
    include_abstention: bool = True,
) -> str:
    """Pick the in-context examples to include for the given tool filter.

    - If the filter is empty (e.g. ToolRAG signaled abstention), return
      ONLY the abstention examples (those with empty `tools_used`). The
      planner should be biased toward join() in this case.
    - If the filter is non-empty, return all examples whose tool set is a
      subset of the filter. The abstention example is included only when
      `include_abstention=True` (the default). Set it to False when
      ToolRAG has narrowed the tool list — we don't want abstention
      demonstrations bleeding into queries where a real tool is available.
    """
    if not available_tool_names:
        return "".join(ex.body for ex in LUPUS_EXAMPLES if not ex.tools_used)
    parts = [
        ex.body
        for ex in LUPUS_EXAMPLES
        if ex.tools_used and ex.tools_used.issubset(available_tool_names)
    ]
    if include_abstention:
        parts.extend(ex.body for ex in LUPUS_EXAMPLES if not ex.tools_used)
    return "".join(parts)


# Verbatim from `src/tiny_agent/prompts.py::get_planner_custom_instructions_prompt`
# minus the Apple-app-specific tool sets and minus the date/meeting-duration
# lines (we don't have time-based tools).
LUPUS_CUSTOM_INSTRUCTIONS = (
    " - You need to start your plan with the '1.' call\n"
    " - Do not use named arguments in your tool calls.\n"
    " - You MUST end your plans with the 'join()' call and a '\\n' character.\n"
    " - You MUST fill every argument in the tool calls, even if they are optional.\n"
    " - If you want to use the result of a previous tool call, you MUST use the '$' sign followed by the index of the tool call.\n"
    " - You MUST ONLY USE join() at the very very end of the plan, or you WILL BE PENALIZED.\n"
)


def build_planner_system_prompt(tools: list[LupusTool]) -> str:
    """Mirror of `generate_llm_compiler_prompt(is_replan=False)` from
    `dist/tinyagent-source/src/llm_compiler/planner.py`.

    Filters the in-context examples to ones whose tools are a subset of
    the provided tool list, so the planner never sees an example calling
    a tool that has been filtered out (important for ToolRAG)."""
    # Numbered list of actions: each tool, then join() at position N+1.
    prefix = (
        "Given a user query, create a plan to solve it with the utmost parallelizability. "
        f"Each plan should comprise an action from the following {len(tools) + 1} types:\n"
    )

    for i, tool in enumerate(tools):
        prefix += f"{i + 1}. {tool.description}\n"
    prefix += f"{len(tools) + 1}. {JOIN_DESCRIPTION}\n\n"

    prefix += (
        "Guidelines:\n"
        " - Each action described above contains input/output types and description.\n"
        "    - You must strictly adhere to the input and output types for each action.\n"
        "    - The action descriptions contain the guidelines. You MUST strictly follow those guidelines when you use the actions.\n"
        " - Each action in the plan should strictly be one of the above types. Follow the Python conventions for each action.\n"
        " - Each action MUST have a unique ID, which is strictly increasing.\n"
        " - Inputs for actions can either be constants or outputs from preceding actions. "
        "In the latter case, use the format $id to denote the ID of the previous action whose output will be the input.\n"
        f" - Always call join as the last action in the plan. Say '{END_OF_PLAN}' after you call join\n"
        " - Ensure the plan maximizes parallelizability.\n"
        " - Only use the provided action types. If a query cannot be addressed using these, invoke the join action for the next steps.\n"
        " - Never explain the plan with comments (e.g. #).\n"
        " - Never introduce new actions other than the ones provided.\n\n"
    )

    prefix += LUPUS_CUSTOM_INSTRUCTIONS + "\n"

    prefix += "Here are some examples:\n\n"
    # When the planner has the full Lupus surface (Step A baseline), include
    # the abstention example. When the surface has been narrowed by ToolRAG
    # to a subset, drop it to avoid biasing the planner toward join().
    is_full_surface = len(tools) >= len(LUPUS_TOOLS)
    prefix += build_in_context_examples(
        {t.name for t in tools},
        include_abstention=is_full_surface,
    )

    return prefix


def main() -> int:
    if not MODEL_GGUF.exists():
        print(f"ERROR: model not found at {MODEL_GGUF}", file=sys.stderr)
        print(
            "Run: hf download squeeze-ai-lab/TinyAgent-1.1B-GGUF "
            "TinyAgent-1.1B-Q4_K_M.gguf --local-dir dist/tinyagent",
            file=sys.stderr,
        )
        return 1

    # Suppress llama.cpp's noisy startup logging.
    os.environ.setdefault("LLAMA_LOG_LEVEL", "ERROR")

    print(f"Loading model: {MODEL_GGUF}")
    t0 = time.time()
    from llama_cpp import Llama  # local import: avoids import cost on errors

    llm = Llama(
        model_path=str(MODEL_GGUF),
        n_ctx=4096,
        n_gpu_layers=0,
        verbose=False,
    )
    print(f"  loaded in {time.time() - t0:.1f}s\n")

    system_prompt = build_planner_system_prompt(LUPUS_TOOLS)
    query = "Find pages about wolves in my local index"
    human_prompt = f"Question: {query}"

    print("=" * 78)
    print("SYSTEM PROMPT (rendered)")
    print("=" * 78)
    print(system_prompt)
    print()
    print("=" * 78)
    print(f"USER PROMPT: {human_prompt}")
    print("=" * 78)
    print()

    # ------------------------------------------------------------------
    # Probe path A: chat completion (uses model's built-in chat template,
    # which for TinyLlama-derived models is the Zephyr style).
    # ------------------------------------------------------------------
    print("--- Probe A: chat completion (system + user message) ---")
    t0 = time.time()
    chat_out = llm.create_chat_completion(
        messages=[
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": human_prompt},
        ],
        max_tokens=512,
        temperature=0.0,
        stop=[END_OF_PLAN, "<|eot_id|>", "</s>", "###"],
    )
    chat_text = chat_out["choices"][0]["message"]["content"]
    print(f"  inference: {time.time() - t0:.1f}s")
    print("  raw model output:")
    print("  " + "-" * 74)
    for line in chat_text.splitlines() or [""]:
        print(f"  | {line}")
    print("  " + "-" * 74)
    print()

    # ------------------------------------------------------------------
    # Probe path B: raw text completion (the BaseLLM path in upstream
    # planner.py — system + "\n\n" + human, no chat template).
    # ------------------------------------------------------------------
    print("--- Probe B: raw text completion (planner.run_llm BaseLLM path) ---")
    raw_prompt = system_prompt + "\n\n" + human_prompt
    t0 = time.time()
    raw_out = llm(
        raw_prompt,
        max_tokens=512,
        temperature=0.0,
        stop=[END_OF_PLAN, "<|eot_id|>", "</s>", "###"],
    )
    raw_text = raw_out["choices"][0]["text"]
    print(f"  inference: {time.time() - t0:.1f}s")
    print("  raw model output:")
    print("  " + "-" * 74)
    for line in raw_text.splitlines() or [""]:
        print(f"  | {line}")
    print("  " + "-" * 74)
    print()

    print("=" * 78)
    print("Phase 1 probe complete. Inspect both outputs above and decide:")
    print("  - Does either path emit `1. search_local_index(...) 2. join()`?")
    print("  - If yes: LLMCompiler premise confirmed. Daemon's JSON marker")
    print("    parser (daemon/src/tools/mod.rs) needs to be replaced.")
    print("  - If no: document the actual format and revisit Phase 2.")
    print("=" * 78)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
