#!/usr/bin/env python3
"""Build the planner LoRA training dataset.

Step C of docs/TINYAGENT_STEPB_FINDINGS.md.

Produces ~380 (user query, LLMCompiler plan) training pairs covering all 6
Lupus tools, multi-step chains with $N references, and abstention against
BAIR's Apple-app-trained prior. Output: datasets/search/planner_train.jsonl
in HF chat-format messages, validated against PlannerExample.

Critical constraints (per docs/TINYAGENT_STEPB_FINDINGS.md):
1. The 22 eval cases in tools/eval_tinyagent.py::TEST_CASES are HELD OUT
   entirely. We assert no training query exactly matches an eval query so
   the eval measures generalization, not memorization.
2. Plans use the LLMCompiler grammar exactly as the planner emits it at
   inference: numbered steps, positional args (no kwargs), $N for
   dependencies, terminating join() followed by <END_OF_PLAN>.
3. The system prompt is NOT stored in the JSONL — the trainer prepends it
   from `tinyagent_prompt_probe.build_planner_system_prompt(LUPUS_TOOLS)`
   so any prompt changes propagate to training automatically.

Why this many examples in this distribution: Step A's eval showed the
remaining failure modes are concentrated in (a) tool confusion on
ambiguous wording, (b) hallucination from BAIR's training prior. The
abstention category is intentionally over-weighted to counter (b).
"""

from __future__ import annotations

import json
import random
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tools"))

from tinyagent_prompt_probe import END_OF_PLAN  # noqa: E402

OUTPUT_PATH = REPO_ROOT / "datasets" / "search" / "planner_train.jsonl"

# Held-out eval queries — must not appear in training data. Imported here so
# any change to the eval cases automatically updates the holdout list.
sys.path.insert(0, str(REPO_ROOT / "tools"))
from eval_tinyagent import TEST_CASES  # noqa: E402

HELD_OUT_QUERIES = {tc.query.strip().lower() for tc in TEST_CASES}


# ---------------------------------------------------------------------------
# Plan rendering helpers
# ---------------------------------------------------------------------------


def make_plan(*calls: tuple[str, str], thought: str | None = None) -> str:
    """Build an LLMCompiler plan from positional (tool_name, args_str) tuples.

    Always appends a final join() with the END_OF_PLAN sentinel. Optionally
    includes a Thought line before the join, mirroring the in-context
    example format used in tinyagent_prompt_probe.LUPUS_EXAMPLES."""
    lines = [f"{i + 1}. {name}({args})" for i, (name, args) in enumerate(calls)]
    if thought:
        lines.append(f"Thought: {thought}")
    lines.append(f"{len(calls) + 1}. join(){END_OF_PLAN}")
    return "\n".join(lines)


def make_abstention_plan(thought: str) -> str:
    """Build a bare join() abstention plan with a Thought explaining why."""
    return f"Thought: {thought}\n1. join(){END_OF_PLAN}"


@dataclass
class TrainingExample:
    id: str
    category: str  # PlannerCategory string
    query: str
    plan: str
    expected_tools: list[str] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Topic vocabularies — kept varied to prevent memorization of specific words
# ---------------------------------------------------------------------------

# Topics for search_local_index queries — things a user might have saved
# pages about. Mix of tech, craft, science, art, philosophy.
LOCAL_TOPICS = [
    "rust ownership rules", "python asyncio patterns", "kubernetes networking",
    "react hooks", "postgres indexing", "vim plugins", "haskell monads",
    "typescript generics", "docker compose", "git rebase strategies",
    "ceramic glazing", "wool felting", "bookbinding", "letterpress printing",
    "indigo dyeing", "natural fiber spinning", "weaving patterns", "leatherwork",
    "wood carving", "blacksmithing basics",
    "Mendelian genetics", "molecular orbitals", "stellar nucleosynthesis",
    "tensor calculus", "category theory", "fluid dynamics",
    "Renaissance perspective drawing", "color theory", "watercolor techniques",
    "cyanotype process", "linocut printing", "etching techniques",
    "stoic ethics", "phenomenology", "Spinoza substance",
    "permaculture design", "mycology field guide", "beekeeping basics",
    "soil microbiology", "regenerative agriculture",
    "Anishinaabe oral tradition", "Welsh folklore", "Norse cosmology",
    "Yoruba creation stories", "Inuit throat singing", "Sami joik",
    "IPFS content routing", "decentralized identity", "hypermedia APIs",
    "homomorphic encryption", "merkle proofs",
]

# Topics for search_subnet queries — cooperative datapods
SUBNET_TOPICS = [
    "regenerative weaving", "open source 3D printing", "permaculture design",
    "natural dye recipes", "community ovens", "tool libraries",
    "seed saving cooperatives", "shared workshops", "land trusts",
    "appropriate technology", "low power computing", "off-grid solar",
    "rainwater harvesting", "passive house design", "cob building",
    "mushroom cultivation", "cottage cheesemaking", "fermentation guilds",
    "open hardware", "reproducible science", "data commons",
    "mesh networks", "federated chat", "git forges",
    "distributed storage", "censorship-resistant publishing",
    "Indigenous land mapping", "language revitalization", "cultural archives",
    "decentralized knowledge graphs", "cooperative housing",
    "worker-owned bakeries", "community currencies", "time banks",
]

# URLs for fetch_page / scan_security / crawl_index queries — diverse and
# fictional so we don't accidentally train against real-world bias
URL_POOL = [
    "https://example.org/page",
    "https://example.com/article.html",
    "https://docs.example.io/guide",
    "https://blog.example.net/post-2024",
    "https://wiki.example.org/Main_Page",
    "https://mirror.example.coop/news",
    "https://commons.example.art/exhibit",
    "https://library.example.edu/catalog",
    "https://forge.example.dev/repo",
    "https://garden.example.farm/journal",
    "https://archive.example.io/2023/06/study",
    "https://hub.example.coop/feed",
    "https://news.example.cafe/today",
    "https://reader.example.fyi/list",
    "https://pad.example.wiki/notes",
    "hvym://cooperative/weaving/intro",
    "hvym://datapods/permaculture-zones",
    "hvym://field/ceramics-handbook",
    "hvym://knowledge/seed-savers",
    "hvym://workshop/blacksmith-basics",
]

# URLs that look "suspicious" for scan_security examples — fictional, never
# real domains. Each is something a phishing scan should plausibly catch.
SUSPICIOUS_URLS = [
    "https://paypa1-secure.support/login",
    "https://app1e-id.verify-account.co/signin",
    "https://amaz0n-prime.delivery-track.io/order",
    "https://goog1e-docs.share-link.com/file",
    "https://faceboook.com.security-check.net/",
    "https://m1crosoft-365.outlook-verify.org/",
    "https://netfllix.account-update.tv/",
    "https://chase-bank.secure-portal.us/",
    "https://gov-tax-refund.federal-claim.gov.us-citizen.co/",
    "https://drophox.share-document.io/",
]


# ---------------------------------------------------------------------------
# Per-category builders
# ---------------------------------------------------------------------------


def build_search_local_index_examples() -> list[TrainingExample]:
    """search_local_index single-tool — mix of wording variants and topics.

    The wordings target the failure modes from Step A: queries with "saved",
    "save", "history", "previously", "I have" — these are the cases that
    Step A's eval failed on (cases 2, 5)."""
    wording_templates = [
        "Find pages in my local index about {topic}",
        "Search my local index for {topic}",
        "What pages have I saved about {topic}",
        "Show me saved articles about {topic}",
        "What did I save about {topic}",
        "Look up {topic} in my saved pages",
        "Search my history for {topic} explanations",
        "Any pages I saved on {topic}",
        "Show me my saved content about {topic}",
        "What's in my local index for {topic}",
        "Find what I previously saved about {topic}",
        "Search my local history for {topic}",
        "What pages have I kept about {topic}",
        "Show me bookmarked articles on {topic}",
    ]
    examples: list[TrainingExample] = []
    rng = random.Random(20260408)
    # Pair each template with ~4 distinct topics, rotated
    for i, template in enumerate(wording_templates):
        for j in range(4):
            topic = LOCAL_TOPICS[(i * 4 + j) % len(LOCAL_TOPICS)]
            query = template.format(topic=topic)
            plan = make_plan(
                ("search_local_index", f'"{topic}", 10'),
                thought="I have searched the local index.",
            )
            examples.append(
                TrainingExample(
                    id=f"sli-{i:02d}-{j}",
                    category="single_tool",
                    query=query,
                    plan=plan,
                    expected_tools=["search_local_index"],
                )
            )
    return examples


def build_search_subnet_examples() -> list[TrainingExample]:
    """search_subnet single-tool — variants emphasizing 'datapod' / 'subnet'
    / 'cooperative' wording cues."""
    wording_templates = [
        "Find datapods about {topic}",
        "Search the cooperative subnet for {topic}",
        "Look up datapods on {topic}",
        "Are there subnet entries for {topic}",
        "Find cooperative resources about {topic}",
        "Search the hvym subnet for {topic}",
        "What datapods exist for {topic}",
        "Find shared resources about {topic}",
        "Search the subnet for {topic} datapods",
        "Look up {topic} in the cooperative subnet",
    ]
    examples: list[TrainingExample] = []
    for i, template in enumerate(wording_templates):
        for j in range(4):
            topic = SUBNET_TOPICS[(i * 4 + j) % len(SUBNET_TOPICS)]
            query = template.format(topic=topic)
            plan = make_plan(
                ("search_subnet", f'"{topic}", ""'),
                thought="I have searched the cooperative subnet for matching datapods.",
            )
            examples.append(
                TrainingExample(
                    id=f"sub-{i:02d}-{j}",
                    category="single_tool",
                    query=query,
                    plan=plan,
                    expected_tools=["search_subnet"],
                )
            )
    return examples


def build_fetch_page_examples() -> list[TrainingExample]:
    """fetch_page single-tool — pure fetch with no extraction. Counterweight
    to the multi-step fetch+extract chain."""
    wording_templates = [
        "Fetch {url}",
        "Get the content of {url}",
        "Download {url}",
        "Retrieve {url}",
        "Fetch the page at {url}",
        "Get {url}",
        "Pull the content from {url}",
        "Load {url}",
    ]
    examples: list[TrainingExample] = []
    for i, template in enumerate(wording_templates):
        for j in range(4):
            url = URL_POOL[(i * 4 + j) % len(URL_POOL)]
            query = template.format(url=url)
            plan = make_plan(
                ("fetch_page", f'"{url}"'),
                thought="I have fetched the page.",
            )
            examples.append(
                TrainingExample(
                    id=f"fp-{i:02d}-{j}",
                    category="single_tool",
                    query=query,
                    plan=plan,
                    expected_tools=["fetch_page"],
                )
            )
    return examples


def build_crawl_index_examples() -> list[TrainingExample]:
    """crawl_index single-tool — 'add to index', 'index this', 'save to'
    wording variants. The Step A eval showed the model defaults to
    fetch_page for these queries; this category should counter-condition that."""
    wording_templates = [
        "Add {url} to my index",
        "Index this page: {url}",
        "Crawl and index {url}",
        "Save {url} to my local index",
        "Index the page at {url}",
        "Add the page {url} to my collection",
        "Bookmark and index {url}",
        "Make a local index entry for {url}",
        "Index this URL: {url}",
        "Crawl {url} into my index",
    ]
    examples: list[TrainingExample] = []
    for i, template in enumerate(wording_templates):
        for j in range(3):
            url = URL_POOL[(i * 3 + j) % len(URL_POOL)]
            query = template.format(url=url)
            plan = make_plan(
                ("crawl_index", f'"{url}"'),
                thought="I have indexed the page.",
            )
            examples.append(
                TrainingExample(
                    id=f"ci-{i:02d}-{j}",
                    category="single_tool",
                    query=query,
                    plan=plan,
                    expected_tools=["crawl_index"],
                )
            )
    return examples


def build_fetch_scan_chain_examples() -> list[TrainingExample]:
    """fetch_page -> scan_security chain. Step A's eval showed the model
    fails on 'is X safe?' / 'check X for threats' wordings — this category
    teaches both."""
    wording_templates = [
        "Is {url} safe?",
        "Check {url} for threats",
        "Is the page at {url} dangerous?",
        "Scan {url} for security issues",
        "Tell me if {url} is safe to visit",
        "Check {url} for phishing",
        "Run a security scan on {url}",
        "Is {url} a phishing site?",
        "Check whether {url} is malicious",
        "Verify {url} is not dangerous",
    ]
    examples: list[TrainingExample] = []
    pool = URL_POOL + SUSPICIOUS_URLS
    for i, template in enumerate(wording_templates):
        for j in range(4):
            url = pool[(i * 4 + j) % len(pool)]
            query = template.format(url=url)
            plan = make_plan(
                ("fetch_page", f'"{url}"'),
                ("scan_security", f'"$1", "{url}"'),
                thought="I have fetched the page and scanned it for security threats.",
            )
            examples.append(
                TrainingExample(
                    id=f"fs-{i:02d}-{j}",
                    category="multi_step",
                    query=query,
                    plan=plan,
                    expected_tools=["fetch_page", "scan_security"],
                )
            )
    return examples


def build_fetch_extract_chain_examples() -> list[TrainingExample]:
    """fetch_page -> extract_content chain with the 2-arg form. Specifically
    targeted at 'summarize' / 'extract content' / 'get the summary' wordings
    so the model doesn't repurpose the format slot for arbitrary keywords."""
    wording_templates = [
        ("Summarize {url}", "summary"),
        ("Get the summary of {url}", "summary"),
        ("Extract the main content from {url}", "full"),
        ("Get a summary of the page at {url}", "summary"),
        ("Summarize the article at {url}", "summary"),
        ("Pull the content from {url} and summarize it", "summary"),
        ("Extract a summary of {url}", "summary"),
        ("Read and summarize {url}", "summary"),
        ("Give me the highlights of {url}", "summary"),
        ("Extract the full text of {url}", "full"),
    ]
    examples: list[TrainingExample] = []
    for i, (template, fmt) in enumerate(wording_templates):
        for j in range(3):
            url = URL_POOL[(i * 3 + j) % len(URL_POOL)]
            query = template.format(url=url)
            plan = make_plan(
                ("fetch_page", f'"{url}"'),
                ("extract_content", f'"$1", "{fmt}"'),
                thought="I have fetched the page and extracted its content.",
            )
            examples.append(
                TrainingExample(
                    id=f"fe-{i:02d}-{j}",
                    category="multi_step",
                    query=query,
                    plan=plan,
                    expected_tools=["fetch_page", "extract_content"],
                )
            )
    return examples


def build_multi_step_chain_examples() -> list[TrainingExample]:
    """Other multi-step patterns: search→fetch with $N, search_subnet→crawl,
    3-tool fetch→extract→scan chains."""
    examples: list[TrainingExample] = []

    # search_local_index -> fetch_page (the $N reference test)
    sl_fetch_templates = [
        "Look up {topic} in my local index, then fetch the first result",
        "Search my saved pages for {topic} and fetch the top match",
        "Find {topic} in my local index then retrieve the first hit",
        "Look up {topic} in my history and pull the first result in full",
        "Search local for {topic} then fetch what comes up first",
    ]
    for i, template in enumerate(sl_fetch_templates):
        for j in range(3):
            topic = LOCAL_TOPICS[(i * 3 + j) % len(LOCAL_TOPICS)]
            query = template.format(topic=topic)
            plan = make_plan(
                ("search_local_index", f'"{topic}", 10'),
                ("fetch_page", '"$1"'),
                thought="I have searched the local index and fetched the first result.",
            )
            examples.append(
                TrainingExample(
                    id=f"slf-{i:02d}-{j}",
                    category="multi_step",
                    query=query,
                    plan=plan,
                    expected_tools=["search_local_index", "fetch_page"],
                )
            )

    # search_subnet -> crawl_index (find a datapod and save it)
    sub_crawl_templates = [
        "Find a datapod about {topic} and save it to my index",
        "Search the subnet for {topic} and add it to my collection",
        "Find cooperative resources about {topic} and index them",
        "Search the subnet for {topic} datapods and crawl them",
        "Find {topic} in the subnet and bookmark it locally",
    ]
    for i, template in enumerate(sub_crawl_templates):
        for j in range(3):
            topic = SUBNET_TOPICS[(i * 3 + j) % len(SUBNET_TOPICS)]
            query = template.format(topic=topic)
            plan = make_plan(
                ("search_subnet", f'"{topic}", ""'),
                ("crawl_index", '"$1"'),
                thought="I have found the datapod and indexed it.",
            )
            examples.append(
                TrainingExample(
                    id=f"sc-{i:02d}-{j}",
                    category="multi_step",
                    query=query,
                    plan=plan,
                    expected_tools=["search_subnet", "crawl_index"],
                )
            )

    # 3-tool: fetch -> extract -> scan
    three_step_templates = [
        "Fetch {url}, summarize it, and check if it's safe",
        "Get {url}, extract a summary, and scan for security threats",
        "Pull {url} and tell me both what it says and whether it's safe",
        "Read {url}, give me a summary, and check it for malware",
        "Fetch and summarize {url}, also tell me if it's dangerous",
    ]
    for i, template in enumerate(three_step_templates):
        for j in range(3):
            url = URL_POOL[(i * 3 + j) % len(URL_POOL)]
            query = template.format(url=url)
            plan = make_plan(
                ("fetch_page", f'"{url}"'),
                ("extract_content", '"$1", "summary"'),
                ("scan_security", f'"$1", "{url}"'),
                thought="I have fetched the page, extracted its summary, and scanned for threats.",
            )
            examples.append(
                TrainingExample(
                    id=f"fes-{i:02d}-{j}",
                    category="multi_step",
                    query=query,
                    plan=plan,
                    expected_tools=["fetch_page", "extract_content", "scan_security"],
                )
            )

    return examples


def build_abstention_examples() -> list[TrainingExample]:
    """Adversarial: BAIR-trained Apple-app intent that should abstain.

    These are intentionally over-weighted (~80 examples) because the
    hallucinations from Step A's eval (compose_email, send_email, summarize,
    create_datapod) all came from the model reaching back to its BAIR
    training distribution. We need a strong counter-conditioning signal."""

    # The thoughts mention WHY no tool fits, mirroring the in-context example
    # format (`Thought: There is no tool available for X, so I cannot...`).

    bait_buckets: list[tuple[str, list[str], str]] = [
        # (category_name, queries, abstention_thought)
        (
            "email",
            [
                "Send an email to John about the meeting",
                "Compose a new email to the team",
                "Email mom",
                "Reply to the latest email from sales",
                "Forward this thread to legal",
                "Draft an email to my advisor",
                "Send an email to support@example.com",
                "Email the cooperative about the upcoming workshop",
                "Compose a message to the mailing list",
            ],
            "There is no tool available for sending email, so I cannot complete this request.",
        ),
        (
            "sms",
            [
                "Text my friend that I'm running late",
                "Send a text message to mom",
                "SMS Sarah about dinner",
                "Send a quick text to the group",
                "Text the address to my partner",
                "Message Alex on iMessage",
                "Send an SMS reminder to the babysitter",
                "Text the link to John",
            ],
            "There is no tool available for sending SMS or text messages, so I cannot complete this request.",
        ),
        (
            "calendar",
            [
                "Schedule a meeting for tomorrow at 3pm",
                "Create a calendar event for Friday",
                "Add a reminder to my calendar for next week",
                "Block out 2 hours on my calendar tomorrow morning",
                "Make an appointment for next Thursday",
                "Add the workshop to my calendar",
                "Schedule a 1:1 with my manager next week",
                "Put a hold on my calendar for the dentist",
            ],
            "There is no tool available for managing calendar events, so I cannot complete this request.",
        ),
        (
            "maps",
            [
                "Show directions to Apple Park",
                "Where is the nearest coffee shop",
                "Find driving directions to the airport",
                "Show me on a map how to get to downtown",
                "What's the route from here to Brooklyn",
                "Give me walking directions to the museum",
                "Open Apple Park in maps",
                "Show me where Times Square is",
            ],
            "There is no tool available for maps or directions, so I cannot complete this request.",
        ),
        (
            "notes",
            [
                "Create a note about the meeting",
                "Add a note to my Apple Notes folder",
                "Open the meeting notes from yesterday",
                "Append this idea to my project notes",
                "Make a new note titled 'shopping list'",
                "Add to my notes about the recipe",
                "Open my notes on the BAIR paper",
            ],
            "There is no tool available for managing notes, so I cannot complete this request.",
        ),
        (
            "reminders",
            [
                "Remind me to call the dentist tomorrow",
                "Set a reminder for Friday at 5pm",
                "Add a reminder to pick up groceries",
                "Remind me to water the plants this weekend",
                "Create a reminder to renew my passport",
                "Set up a daily reminder to stretch",
            ],
            "There is no tool available for setting reminders, so I cannot complete this request.",
        ),
        (
            "contacts",
            [
                "Get the phone number for John",
                "What's Sarah's email address",
                "Find Alex's contact info",
                "Look up the phone number for the dentist",
                "Get my mom's email from contacts",
                "Find the email for support",
            ],
            "There is no tool available for looking up contacts, so I cannot complete this request.",
        ),
        (
            "zoom",
            [
                "Set up a zoom meeting for tomorrow at 2pm",
                "Create a zoom link for the workshop",
                "Schedule a zoom call with the team",
                "Make a zoom meeting for the cooperative",
            ],
            "There is no tool available for creating Zoom meetings, so I cannot complete this request.",
        ),
        (
            "phone",
            [
                "Call mom",
                "Dial 911",
                "Place a call to the dentist",
                "Call the airline customer service",
            ],
            "There is no tool available for making phone calls, so I cannot complete this request.",
        ),
        (
            "shell",
            [
                "Open a terminal and run ls",
                "Run npm install in the project directory",
                "Execute the build script",
                "Run pytest on the test suite",
                "Cat the contents of /etc/hosts",
                "Show me the running processes",
            ],
            "There is no tool available for executing shell commands, so I cannot complete this request.",
        ),
        (
            "translation",
            [
                "Translate this French sentence to German: bonjour le monde",
                "How do you say 'thank you' in Japanese",
                "Translate the Spanish phrase 'hola amigo'",
                "What does 'arigato' mean in English",
                "Translate this paragraph to Mandarin",
            ],
            "There is no tool available for language translation, so I cannot complete this request.",
        ),
        (
            "general_knowledge",
            [
                "What is the capital of France",
                "How many planets are in the solar system",
                "What year did World War 2 end",
                "Who wrote Hamlet",
                "What is the speed of light",
                "What is 2 plus 2",
                "How many feet in a mile",
                "Who are you",
                "What can you do",
                "Tell me a joke",
            ],
            "I can answer this directly without calling any tools.",
        ),
    ]

    examples: list[TrainingExample] = []
    counter = 0
    for bucket_name, queries, thought in bait_buckets:
        for q in queries:
            examples.append(
                TrainingExample(
                    id=f"abst-{bucket_name}-{counter:03d}",
                    category="abstention",
                    query=q,
                    plan=make_abstention_plan(thought),
                    expected_tools=[],
                )
            )
            counter += 1
    return examples


# ---------------------------------------------------------------------------
# Assembly + validation + write
# ---------------------------------------------------------------------------


def assemble_all() -> list[TrainingExample]:
    examples: list[TrainingExample] = []
    examples += build_search_local_index_examples()
    examples += build_search_subnet_examples()
    examples += build_fetch_page_examples()
    examples += build_crawl_index_examples()
    examples += build_fetch_scan_chain_examples()
    examples += build_fetch_extract_chain_examples()
    examples += build_multi_step_chain_examples()
    examples += build_abstention_examples()
    return examples


def assert_no_holdout_collisions(examples: list[TrainingExample]) -> None:
    """Verify no training query exactly matches a held-out eval query.

    Strict: case-insensitive whole-string match. Substring overlap is OK
    (different topic in the same template) but exact match is forbidden."""
    collisions: list[tuple[str, str]] = []
    for ex in examples:
        if ex.query.strip().lower() in HELD_OUT_QUERIES:
            collisions.append((ex.id, ex.query))
    if collisions:
        print(
            f"ERROR: {len(collisions)} training examples collide with held-out eval queries:",
            file=sys.stderr,
        )
        for example_id, query in collisions:
            print(f"  {example_id}: {query!r}", file=sys.stderr)
        sys.exit(1)


def to_planner_example_dict(ex: TrainingExample) -> dict:
    return {
        "id": ex.id,
        "category": ex.category,
        "messages": [
            {"role": "user", "content": f"Question: {ex.query}"},
            {"role": "assistant", "content": ex.plan},
        ],
        "expected_tools": ex.expected_tools,
        "metadata": {},
    }


def write_jsonl(examples: list[TrainingExample], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as fp:
        for ex in examples:
            fp.write(json.dumps(to_planner_example_dict(ex)) + "\n")


def validate_with_pydantic(path: Path) -> int:
    """Validate every line of the JSONL against PlannerExample. Returns
    the count of valid examples; exits non-zero on any failure."""
    sys.path.insert(0, str(REPO_ROOT))
    from datasets.search.schema import PlannerExample  # noqa: E402

    count = 0
    errors: list[str] = []
    for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            continue
        try:
            data = json.loads(line)
            PlannerExample.model_validate(data)
            count += 1
        except Exception as e:
            errors.append(f"line {line_no}: {e}")
    if errors:
        print(f"ERROR: {len(errors)} schema validation failures", file=sys.stderr)
        for err in errors[:5]:
            print(f"  {err}", file=sys.stderr)
        sys.exit(1)
    return count


def print_summary(examples: list[TrainingExample]) -> None:
    by_category: dict[str, int] = {}
    by_tool: dict[str, int] = {}
    abstention_buckets: dict[str, int] = {}
    for ex in examples:
        by_category[ex.category] = by_category.get(ex.category, 0) + 1
        for t in ex.expected_tools:
            by_tool[t] = by_tool.get(t, 0) + 1
        if ex.category == "abstention":
            bucket = ex.id.split("-")[1] if ex.id.startswith("abst-") else "other"
            abstention_buckets[bucket] = abstention_buckets.get(bucket, 0) + 1

    print(f"\nTotal examples: {len(examples)}\n")
    print("By category:")
    for cat, n in sorted(by_category.items(), key=lambda kv: -kv[1]):
        print(f"  {cat:<20} {n:>4}")
    print("\nBy tool (multi-tool examples count once per tool):")
    for tool, n in sorted(by_tool.items(), key=lambda kv: -kv[1]):
        print(f"  {tool:<22} {n:>4}")
    print("\nAbstention buckets:")
    for bucket, n in sorted(abstention_buckets.items(), key=lambda kv: -kv[1]):
        print(f"  {bucket:<22} {n:>4}")


def main() -> int:
    examples = assemble_all()
    assert_no_holdout_collisions(examples)
    write_jsonl(examples, OUTPUT_PATH)
    valid_count = validate_with_pydantic(OUTPUT_PATH)
    if valid_count != len(examples):
        print(
            f"ERROR: Pydantic validated {valid_count} but built {len(examples)}",
            file=sys.stderr,
        )
        return 1
    print_summary(examples)
    print(f"\nWrote {valid_count} examples to {OUTPUT_PATH}")
    print(f"  size: {OUTPUT_PATH.stat().st_size:,} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
