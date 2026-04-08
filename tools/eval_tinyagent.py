#!/usr/bin/env python3
"""TinyAgent eval — Phases 3, 4, 5 of docs/TINYAGENT_EVAL.md.

Runs the 22 hand-curated Lupus queries against the local TinyAgent-1.1B
GGUF, parses the LLMCompiler plan from each output, scores six metrics
per Phase 5, prints an OK/X/? table, writes raw JSONL to
`eval/tinyagent_runs/<timestamp>.jsonl`, and exits non-zero on hard-metric
failure.

Reuses the rendered prompt scaffold from `tools/tinyagent_prompt_probe.py`
so this script and the probe stay in lock-step. If you change the system
prompt or the rendered tool list, you only change it in the probe.
"""

from __future__ import annotations

import ast
import json
import os
import re
import sys
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tools"))

from tinyagent_prompt_probe import (  # noqa: E402
    END_OF_PLAN,
    LUPUS_TOOLS,
    MODEL_GGUF,
    build_planner_system_prompt,
)
from lupus_tool_rag import LupusToolRAG  # noqa: E402

# ToolRAG disabled by default — Step B finding (docs/TINYAGENT_STEPB_FINDINGS.md):
# the all-MiniLM-L6-v2 embedding model is too lexical for our 6-tool surface
# and consistently routes "saved articles" / "local history" / "pages mentioning"
# to the wrong tool. ToolRAG mistakes cascade because removing the right tool
# from the planner's view is unrecoverable. Set this to True only when
# experimenting with a different embedding model or a tuned tool surface.
USE_TOOL_RAG = False
TOOL_RAG_TOP_K = 3
TOOL_RAG_PRIMARY_THRESHOLD = 0.10  # top match must clear this for non-abstention
TOOL_RAG_SECONDARY_THRESHOLD = 0.04  # other tools in top_k must clear this

RUNS_DIR = REPO_ROOT / "eval" / "tinyagent_runs"

# Tool names that terminate a plan. The model is supposed to emit `join()`
# but it sometimes hallucinates `join_finish(...)` / `join_replan(...)` from
# its training distribution; treat them all as terminators for parsing
# purposes (the scorer flags non-`join` variants separately).
JOIN_NAMES = {"join", "join_finish", "join_replan"}

LUPUS_TOOL_NAMES = {t.name for t in LUPUS_TOOLS}

# Positional-arg arity for each tool. We don't enforce types here because
# every arg in the LLMCompiler grammar is either a string literal, an int
# literal, or a `$N` placeholder that will be resolved at execution time.
# Arity is the only thing the model can reliably get wrong on its own.
TOOL_ARITY: dict[str, int] = {
    "search_subnet": 2,         # query, scope
    "search_local_index": 2,    # query, top_k
    "fetch_page": 1,            # url
    "extract_content": 2,       # html, format
    "scan_security": 2,         # html, url
    "crawl_index": 1,           # source
}


# ---------------------------------------------------------------------------
# Test cases (Phase 3 of docs/TINYAGENT_EVAL.md, lines 142-184).
# ---------------------------------------------------------------------------


@dataclass
class TestCase:
    id: int
    query: str
    expected_tools: set[str]
    multi_step: bool = False
    allow_abstain: bool = False
    requires_dependency: bool = False  # plan must use $N to chain steps
    notes: str = ""
    # Optional alternative tool sets that should also be considered correct
    # for tool-selection scoring (e.g. case 11 can fetch+scan or scan-only).
    acceptable_alt_tools: list[set[str]] = field(default_factory=list)


TEST_CASES: list[TestCase] = [
    # ---- Single-tool: search_local_index (5) ----
    TestCase(1, "Find pages about wolves in my local index", {"search_local_index"}),
    TestCase(2, "What did I save about Anishinaabe folklore?", {"search_local_index"}),
    TestCase(3, "Search my local history for rust borrow checker explanations", {"search_local_index"}),
    TestCase(4, "Any pages mentioning IPFS content routing?", {"search_local_index"}),
    TestCase(5, "Show me saved articles about wool felting", {"search_local_index"}),
    # ---- Single-tool: search_subnet (3) ----
    TestCase(6, "Search the hvym cooperative for weaving datapods", {"search_subnet"}),
    TestCase(7, "Find datapods about open-source 3D printing", {"search_subnet"}),
    TestCase(8, "Is there a subnet entry for Lepus browser docs?", {"search_subnet"}),
    # ---- Single-tool: fetch_page (2) ----
    TestCase(9, "Fetch https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/", {"fetch_page"}),
    TestCase(10, "Get the content of hvym://cooperative/weaving/intro", {"fetch_page"}),
    # ---- Single-tool: scan_security (2) ----
    TestCase(
        11,
        "Is https://paypa1-secure.support/login.php safe?",
        {"scan_security"},
        notes="HTML not given; model may also choose fetch_page → scan_security",
        acceptable_alt_tools=[{"fetch_page", "scan_security"}],
    ),
    TestCase(
        12,
        "Check https://github.com/inviti8/lupus for threats",
        {"scan_security"},
        acceptable_alt_tools=[{"fetch_page", "scan_security"}],
    ),
    # ---- Single-tool: crawl_index (1) ----
    TestCase(13, "Add https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/ to my index", {"crawl_index"}),
    # ---- Multi-step (5) ----
    TestCase(
        14,
        "Summarize the BAIR TinyAgent blog post",
        {"fetch_page", "extract_content"},
        multi_step=True,
        requires_dependency=True,
    ),
    TestCase(
        15,
        "Fetch https://example.com/article.html and tell me if it's dangerous",
        {"fetch_page", "scan_security"},
        multi_step=True,
        requires_dependency=True,
    ),
    TestCase(
        16,
        "Find a cooperative datapod about weaving and save it to my index",
        {"search_subnet", "crawl_index"},
        multi_step=True,
        requires_dependency=True,
    ),
    TestCase(
        17,
        "Look up wolves in my local index, then fetch the first result in full",
        {"search_local_index", "fetch_page"},
        multi_step=True,
        requires_dependency=True,
        notes="critical $N LLMCompiler test",
    ),
    TestCase(
        18,
        "Fetch the Lupus GitHub README, extract its summary, and scan it for threats",
        {"fetch_page", "extract_content", "scan_security"},
        multi_step=True,
        requires_dependency=True,
    ),
    # ---- No-tool / abstention (2) ----
    TestCase(19, "What is 2+2?", set(), allow_abstain=True),
    TestCase(20, "Who are you?", set(), allow_abstain=True),
    # ---- Adversarial / hallucination check (2) ----
    TestCase(
        21,
        "Email my wife that I'll be late",
        set(),
        allow_abstain=True,
        notes="no email tool exists; calling fabricated send_email is the headline failure",
    ),
    TestCase(
        22,
        "Open a terminal and run `ls`",
        set(),
        allow_abstain=True,
    ),
]


# ---------------------------------------------------------------------------
# LLMCompiler plan parser.
#
# Mirrors `dist/tinyagent-source/src/llm_compiler/output_parser.py`:
#   ACTION_PATTERN  = r"\s*\n*(\d+)\. (\w+)\((.*)\)(\s*#\w+\n)?"
#   THOUGHT_PATTERN = r"Thought: ([^\n]*)"
#   ID_PATTERN      = r"\$\{?(\d+)\}?"
# We use a slightly looser line-by-line scan because the model sometimes
# omits the space after the period or wraps args across lines.
# ---------------------------------------------------------------------------

# Allow optional leading punctuation/whitespace before the step number, since
# the model sometimes emits a continuation token (e.g. ". 1. search_local_index(...)"
# on the same line). Also allow optional trailing comment.
ACTION_RE = re.compile(r"(?:^|[^\w\$])(\d+)\.\s*(\w+)\s*\((.*?)\)\s*(?:#.*)?\s*$")
ID_REF_RE = re.compile(r"\$\{?(\d+)\}?")


@dataclass
class ParsedCall:
    idx: int
    name: str
    raw_args: str
    parsed_args: tuple[Any, ...]
    references: list[int]  # $N indices found in raw_args

    @property
    def is_join(self) -> bool:
        return self.name in JOIN_NAMES


def _parse_args_string(raw: str) -> tuple[Any, ...]:
    """Parse the comma-separated arg string of a single tool call.

    Mirrors `_parse_llm_compiler_action_args` from upstream — uses
    ast.literal_eval if possible, falls back to the raw string. We wrap
    the args in a synthetic tuple `(...)` so single-arg calls don't lose
    their tuple-ness."""
    raw = raw.strip()
    if not raw:
        return ()
    # Replace $N with literal "$N" strings so ast can parse them.
    safe = ID_REF_RE.sub(lambda m: f'"${m.group(1)}"', raw)
    try:
        parsed = ast.literal_eval(f"({safe},)")
        if isinstance(parsed, tuple):
            return parsed
        return (parsed,)
    except (SyntaxError, ValueError):
        return (raw,)


def parse_plan(text: str) -> list[ParsedCall]:
    """Extract a list of ParsedCall from the model's raw output.

    The model sometimes prepends a continuation token (e.g. a leading
    `. ` or `. \\n`) to its first step, so we don't anchor strictly to
    the line start — we accept the step number after any non-word,
    non-`$` lead-in. This still rejects accidental matches inside string
    literals because step numbers are followed by `. <ident>(`."""
    calls: list[ParsedCall] = []
    for line in text.splitlines():
        m = ACTION_RE.search(line)
        if not m:
            continue
        idx = int(m.group(1))
        name = m.group(2)
        raw_args = m.group(3)
        parsed_args = _parse_args_string(raw_args)
        references = [int(r) for r in ID_REF_RE.findall(raw_args)]
        calls.append(
            ParsedCall(
                idx=idx,
                name=name,
                raw_args=raw_args,
                parsed_args=parsed_args,
                references=references,
            )
        )
        if name in JOIN_NAMES:
            break
    return calls


# ---------------------------------------------------------------------------
# Per-case scorer (Phase 5 metrics)
# ---------------------------------------------------------------------------


@dataclass
class CaseScore:
    case_id: int
    query: str
    raw_output: str
    parsed: list[dict[str, Any]]
    observed_tools: list[str]  # ordered list of non-join tool names
    valid_syntax: bool
    tool_selection_ok: bool
    arg_shape_ok: bool
    hallucinated: bool
    multi_step_ok: bool
    abstained_correctly: bool
    dependency_ok: bool
    notes: list[str]


def score_case(case: TestCase, raw_output: str, parsed: list[ParsedCall]) -> CaseScore:
    notes: list[str] = []
    non_join = [c for c in parsed if not c.is_join]
    observed = [c.name for c in non_join]
    observed_set = set(observed)

    # 1. Syntactic validity: at least one matching call OR valid abstention.
    has_terminator = any(c.is_join for c in parsed)
    if case.allow_abstain:
        valid_syntax = (len(non_join) == 0 and has_terminator) or len(non_join) > 0
    else:
        valid_syntax = len(parsed) > 0 and (has_terminator or len(non_join) > 0)
    if not has_terminator:
        notes.append("no join() terminator")

    # 2. Tool selection accuracy.
    accepted_sets = [case.expected_tools, *case.acceptable_alt_tools]
    tool_selection_ok = any(observed_set == s for s in accepted_sets)

    # 3. Arg shape validity (arity match against TOOL_ARITY).
    arg_shape_ok = True
    for call in non_join:
        if call.name not in TOOL_ARITY:
            continue  # hallucination is scored separately
        expected_arity = TOOL_ARITY[call.name]
        if len(call.parsed_args) != expected_arity:
            arg_shape_ok = False
            notes.append(
                f"{call.name} arity {len(call.parsed_args)} != {expected_arity}"
            )

    # 4. Hallucinated tool rate.
    hallucinated_calls = [
        c for c in non_join if c.name not in LUPUS_TOOL_NAMES
    ]
    hallucinated = len(hallucinated_calls) > 0
    if hallucinated:
        notes.append(
            "hallucinated: " + ", ".join(sorted({c.name for c in hallucinated_calls}))
        )
    # Also flag join_finish / join_replan; the daemon parser will need to
    # know about them but they're a separate kind of hallucination.
    join_variants = [c.name for c in parsed if c.is_join and c.name != "join"]
    if join_variants:
        notes.append("join variant: " + ",".join(join_variants))

    # 5. Multi-step correctness — only meaningful when case.multi_step.
    if case.multi_step:
        multi_step_ok = len(non_join) >= 2
        if case.requires_dependency:
            has_dep = any(c.references for c in non_join)
            dependency_ok = has_dep
            if not has_dep:
                notes.append("no $N dependency reference")
        else:
            dependency_ok = True
    else:
        multi_step_ok = True
        dependency_ok = True

    # 6. Abstention correctness — only meaningful when case.allow_abstain.
    if case.allow_abstain:
        abstained_correctly = len(non_join) == 0
        if not abstained_correctly:
            notes.append("called tools when it should have abstained")
    else:
        abstained_correctly = True

    return CaseScore(
        case_id=case.id,
        query=case.query,
        raw_output=raw_output,
        parsed=[asdict(_pc_to_jsonable(c)) for c in parsed],
        observed_tools=observed,
        valid_syntax=valid_syntax,
        tool_selection_ok=tool_selection_ok,
        arg_shape_ok=arg_shape_ok,
        hallucinated=hallucinated,
        multi_step_ok=multi_step_ok,
        abstained_correctly=abstained_correctly,
        dependency_ok=dependency_ok,
        notes=notes,
    )


def _pc_to_jsonable(c: ParsedCall) -> ParsedCall:
    """Force parsed_args to be JSON-serializable."""
    return ParsedCall(
        idx=c.idx,
        name=c.name,
        raw_args=c.raw_args,
        parsed_args=tuple(repr(a) for a in c.parsed_args),
        references=c.references,
    )


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def case_marker(case: TestCase, s: CaseScore) -> str:
    """ok / X / ? per the test_security_model.py convention."""
    hard_pass = (
        s.valid_syntax
        and s.tool_selection_ok
        and s.arg_shape_ok
        and not s.hallucinated
        and s.multi_step_ok
        and s.abstained_correctly
        and s.dependency_ok
    )
    if hard_pass:
        return "ok"
    if s.valid_syntax and s.tool_selection_ok:
        return "?"  # picked the right tools but tripped on a soft check
    return "X"


def color_thresholds(name: str, value: float) -> str:
    """Return a green/yellow/red label per the Phase 5 thresholds."""
    if name == "tool_selection":
        if value >= 0.80:
            return "GREEN"
        if value >= 0.60:
            return "YELLOW"
        return "RED"
    if name == "arg_shape":
        if value >= 0.90:
            return "GREEN"
        if value >= 0.75:
            return "YELLOW"
        return "RED"
    if name == "hallucination":
        if value <= 0.02:
            return "GREEN"
        if value <= 0.05:
            return "YELLOW"
        return "RED"
    if name == "multi_step":
        if value >= 0.60:
            return "GREEN"
        if value >= 0.40:
            return "YELLOW"
        return "RED"
    if name == "abstention":
        if value >= 0.75:
            return "GREEN"
        if value >= 0.50:
            return "YELLOW"
        return "RED"
    if name == "syntactic_validity":
        if value >= 0.90:
            return "GREEN"
        if value >= 0.70:
            return "YELLOW"
        return "RED"
    return ""


# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------


def main() -> int:
    if not MODEL_GGUF.exists():
        print(f"ERROR: model not found at {MODEL_GGUF}", file=sys.stderr)
        return 1

    os.environ.setdefault("LLAMA_LOG_LEVEL", "ERROR")
    print(f"Loading model: {MODEL_GGUF}")
    t0 = time.time()
    from llama_cpp import Llama  # local import: avoids cost on errors

    llm = Llama(
        model_path=str(MODEL_GGUF),
        n_ctx=4096,
        n_gpu_layers=0,
        verbose=False,
    )
    print(f"  loaded in {time.time() - t0:.1f}s\n")

    tool_rag: LupusToolRAG | None = None
    if USE_TOOL_RAG:
        print("Loading ToolRAG (Sentence-Transformers)...")
        t0 = time.time()
        tool_rag = LupusToolRAG()
        print(
            f"  loaded in {time.time() - t0:.1f}s  "
            f"(top_k={TOOL_RAG_TOP_K}, primary={TOOL_RAG_PRIMARY_THRESHOLD}, "
            f"secondary={TOOL_RAG_SECONDARY_THRESHOLD})\n"
        )

    # If ToolRAG is off we can build the system prompt once and reuse it.
    base_system_prompt = build_planner_system_prompt(LUPUS_TOOLS)
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    jsonl_path = RUNS_DIR / f"{timestamp}-eval.jsonl"

    print(f"Running {len(TEST_CASES)} cases. Raw outputs -> {jsonl_path}\n")
    print(f"{'id':>3}  {'mark':<4}  {'expected':<28}  {'observed':<32}  query")
    print("-" * 120)

    scores: list[CaseScore] = []
    cumulative_inference = 0.0

    with jsonl_path.open("w", encoding="utf-8") as fp:
        for case in TEST_CASES:
            human_prompt = f"Question: {case.query}"

            # Per-query system prompt if ToolRAG is on. The filtered tool
            # list shrinks the planner's candidate space; an empty list means
            # ToolRAG saw nothing relevant and the planner should abstain.
            if tool_rag is not None:
                filtered_tools = tool_rag.retrieve(
                    case.query,
                    top_k=TOOL_RAG_TOP_K,
                    primary_threshold=TOOL_RAG_PRIMARY_THRESHOLD,
                    secondary_threshold=TOOL_RAG_SECONDARY_THRESHOLD,
                )
                if filtered_tools:
                    system_prompt = build_planner_system_prompt(filtered_tools)
                else:
                    # No tool scored above threshold — feed an empty surface
                    # so the planner has to abstain via join().
                    system_prompt = build_planner_system_prompt([])
                rag_filter_names = [t.name for t in filtered_tools]
            else:
                system_prompt = base_system_prompt
                rag_filter_names = None

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
            elapsed = time.time() - t0
            cumulative_inference += elapsed
            raw_text = chat_out["choices"][0]["message"]["content"]

            parsed = parse_plan(raw_text)
            score = score_case(case, raw_text, parsed)
            scores.append(score)

            mark = case_marker(case, score)
            expected_str = ",".join(sorted(case.expected_tools)) or "(abstain)"
            observed_str = ",".join(score.observed_tools) or "(none)"
            rag_str = (
                f"  rag={','.join(rag_filter_names) or '(empty)'}"
                if rag_filter_names is not None else ""
            )
            print(
                f"{case.id:>3}  {mark:<4}  {expected_str:<28}  {observed_str:<32}  {case.query[:50]}{rag_str}"
            )

            fp.write(
                json.dumps(
                    {
                        "case": asdict_nosets(case),
                        "raw_output": raw_text,
                        "parsed": [asdict(_pc_to_jsonable(c)) for c in parsed],
                        "score": _score_to_jsonable(score),
                        "rag_filter": rag_filter_names,
                        "elapsed_sec": elapsed,
                    }
                )
                + "\n"
            )

    print()
    print(f"Total inference time: {cumulative_inference:.1f}s")

    # Aggregate metrics
    total = len(scores)
    syntactic = sum(s.valid_syntax for s in scores) / total
    tool_selection = sum(s.tool_selection_ok for s in scores) / total
    arg_shape = sum(s.arg_shape_ok for s in scores) / total
    hallucination = sum(s.hallucinated for s in scores) / total

    multi_cases = [s for s, c in zip(scores, TEST_CASES) if c.multi_step]
    multi_step = (
        sum(s.multi_step_ok and s.dependency_ok for s in multi_cases) / len(multi_cases)
        if multi_cases else 1.0
    )

    abstain_cases = [s for s, c in zip(scores, TEST_CASES) if c.allow_abstain]
    abstention = (
        sum(s.abstained_correctly for s in abstain_cases) / len(abstain_cases)
        if abstain_cases else 1.0
    )

    print()
    print("=" * 78)
    print("Phase 5 metrics")
    print("=" * 78)
    print(f"  Syntactic validity rate    {syntactic:6.1%}  [{color_thresholds('syntactic_validity', syntactic)}]")
    print(f"  Tool selection accuracy    {tool_selection:6.1%}  [{color_thresholds('tool_selection', tool_selection)}]")
    print(f"  Argument shape validity    {arg_shape:6.1%}  [{color_thresholds('arg_shape', arg_shape)}]")
    print(f"  Hallucinated tool rate     {hallucination:6.1%}  [{color_thresholds('hallucination', hallucination)}]")
    print(f"  Multi-step correctness     {multi_step:6.1%}  [{color_thresholds('multi_step', multi_step)}]  ({len(multi_cases)} cases)")
    print(f"  Abstention correctness     {abstention:6.1%}  [{color_thresholds('abstention', abstention)}]  ({len(abstain_cases)} cases)")
    print()

    ok_count = sum(1 for s, c in zip(scores, TEST_CASES) if case_marker(c, s) == "ok")
    print(f"Hard pass (ok): {ok_count}/{total}")

    print()
    print(f"Raw run: {jsonl_path}")
    print()
    print("Now consult the Phase 6 decision tree in docs/TINYAGENT_EVAL.md")
    print("with the metrics above to pick the next action.")

    # Hard-fail thresholds: syntactic_validity must be at least YELLOW (>=70%)
    # AND tool_selection must be at least YELLOW (>=60%) for the eval to be
    # considered actionable. If those are RED, exit 2 so CI/scripts notice.
    if syntactic < 0.70 or tool_selection < 0.60:
        print("\nHARD FAIL: syntactic validity or tool selection below YELLOW.")
        return 2
    return 0


def asdict_nosets(case: TestCase) -> dict[str, Any]:
    """dataclasses.asdict can't serialize sets — convert them to sorted lists."""
    d = asdict(case)
    d["expected_tools"] = sorted(case.expected_tools)
    d["acceptable_alt_tools"] = [sorted(s) for s in case.acceptable_alt_tools]
    return d


def _score_to_jsonable(s: CaseScore) -> dict[str, Any]:
    return {
        "case_id": s.case_id,
        "query": s.query,
        "observed_tools": s.observed_tools,
        "valid_syntax": s.valid_syntax,
        "tool_selection_ok": s.tool_selection_ok,
        "arg_shape_ok": s.arg_shape_ok,
        "hallucinated": s.hallucinated,
        "multi_step_ok": s.multi_step_ok,
        "abstained_correctly": s.abstained_correctly,
        "dependency_ok": s.dependency_ok,
        "notes": s.notes,
    }


if __name__ == "__main__":
    raise SystemExit(main())
