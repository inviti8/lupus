# Search Adapter Training Dataset

Training examples for the Lupus search adapter — a LoRA fine-tuning dataset for the TinyAgent-1.1B base model. Teaches the model to handle queries by selecting tools, calling functions, and (when relevant) demonstrating cultural knowledge alongside tool calls.

## Structure

```
datasets/search/
  README.md
  schema.py                       ← Pydantic models — single source of truth
  examples/
    tool_calling.jsonl            ← Pure function-calling examples
    multi_step.jsonl              ← Multi-step reasoning with multiple tool calls
    knowledge_aware.jsonl         ← Knowledge + tool calling (incl. folklore-derived)
    no_tool.jsonl                 ← Queries that don't need tools
  derive_from_folklore.py         ← (future) Compendium → knowledge_aware.jsonl
```

## Example categories

| Category | Purpose | Target volume | What it teaches |
|----------|---------|---------------|----------------|
| `tool_calling` | Pure function-calling fluency | ~5000 | Map queries to tool calls with correct arguments |
| `multi_step` | Sequential tool use | ~2000 | Chain tool calls (search → fetch → extract) |
| `knowledge_aware` | Cultural knowledge + tools | ~1500 | Demonstrate domain knowledge alongside searches |
| `no_tool` | Direct responses | ~1500 | Recognize when tools aren't needed |
| **Total** | | **~10000** | |

## Schema

See `schema.py` for canonical Pydantic definitions. Every example matches `SearchExample`:

```python
class SearchExample(BaseModel):
    id: str                      # unique identifier
    category: ExampleCategory     # one of the four categories above
    user_query: str               # what the user asks
    assistant_response: str       # the response, with embedded tool call markers
    expected_tool_calls: list[ExpectedToolCall]  # for evaluation
    metadata: dict                # category-specific tags
```

## Tool call format

The TinyAgent function-calling format uses delimiter markers in the assistant response:

```
<|function_call|>{"name": "search_local_index", "arguments": {"query": "..."}}<|end_function_call|>
```

The model learns to emit these markers when it wants to call a tool. The Lupus daemon parses them out (see `daemon/src/tools/mod.rs::parse_tool_calls`) and dispatches the actual tool execution.

## Available tools (from daemon)

The model is trained to call these tools, defined in `daemon/src/tools/`:

| Tool | Purpose |
|------|---------|
| `search_subnet` | Search cooperative subnet for matching datapod metadata |
| `search_local_index` | Search the local semantic index for previously visited pages |
| `fetch_page` | Fetch page content by URL (hvym or https) |
| `extract_content` | Extract clean text, title, summary, keywords from HTML |
| `scan_security` | Scan HTML + URL for security threats |
| `crawl_index` | Fetch content by CID or URL and create a local index entry |

## Folklore integration

The `knowledge_aware.jsonl` file is derived from the cultural compendium at `datasets/folklore/tales/`. Each `FolkloreTale` in the compendium can produce multiple `SearchExample` entries via different query angles:

- Direct knowledge query — "Tell me about X"
- Character-focused query — "Who is Ma'iingan in Anishinaabe tradition?"
- Theme-focused query — "What stories feature wolf-hare companionship?"
- Comparative query — "How is the moon hare depicted across Asian traditions?"
- Search-flavored query — "Find articles about Nanabozho"

See `datasets/folklore/README.md` for the cultural source material details and ethics.

## File format

JSONL — one JSON object per line. Append-only. Examples are not deduplicated automatically; the `id` field must be unique. Use `python -c "import json; [json.loads(l) for l in open('examples/...')]"` to validate.

## Generation method

Examples are generated directly in Claude Code sessions, validated against the Pydantic schema, and appended to the appropriate JSONL file. No API calls required — uses the maintainer's Claude Code subscription. See `docs/TRAINING_STRATEGY.md` for the full rationale.
