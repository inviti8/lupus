# Lupus Daemon — Architecture

Lupus runs as a standalone daemon process alongside the Lepus browser. The browser communicates with Lupus over a local WebSocket connection. This keeps the browser lean and allows Lupus to develop independently.

---

## Why a Separate Process

| Concern | Embedded (in browser) | Separate Daemon |
|---------|----------------------|-----------------|
| Browser binary size | +50MB ML runtime, +50MB IPFS | No change to browser |
| Memory isolation | ML competes with rendering | Independent memory pools |
| Release cadence | Rebuild Firefox for every model update | Update Lupus independently |
| Crash isolation | Model crash kills browser tab | Model crash doesn't affect browsing |
| Reusability | Only Lepus can use it | Andromica, Metavinci can also connect |
| Development | C++ browser engine complexity | Rust/Python, natural fit for ML + IPFS |
| Tool development | Tools buried in browser code | Tools co-evolve with model training |

---

## Architecture

```
┌─────────────────────────┐     WebSocket      ┌──────────────────────────────┐
│     Lepus Browser       │◄───(localhost)─────►│       Lupus Daemon           │
│                         │                     │                              │
│  URL bar search ───────►│    {"query":...}    │  Agent (TinyAgent + LoRA)    │
│  Trust indicator ◄──────│    {"score":92}     │  Security Model              │
│  Result display ◄───────│    {"results":[]}   │  IPFS Client (Iroh)          │
│  Pelt system            │                     │  Crawler / Indexer           │
│  Subnet selector        │                     │  The Den (local content +    │
│                         │                     │           semantic index)    │
│                         │                     │  Tool Implementations        │
└─────────────────────────┘                     └──────────────────────────────┘
```

---

## IPC Protocol

JSON messages over WebSocket on `ws://localhost:9549` (Lupus default port).

### Request Format

```json
{
  "id": "req-001",
  "method": "search",
  "params": {
    "query": "digital art preservation",
    "scope": "hvym"
  }
}
```

### Response Format

```json
{
  "id": "req-001",
  "status": "ok",
  "result": {
    "results": [
      {
        "title": "Art Preservation Guide",
        "url": "alice@articles/guide",
        "summary": "Comprehensive guide to...",
        "trust_score": 92,
        "commitment": 0.87
      }
    ]
  }
}
```

### Methods

| Method | Direction | Description |
|--------|-----------|-------------|
| `search` | Browser → Lupus | Run a search query via TinyAgent |
| `scan_page` | Browser → Lupus | Scan HTML for security threats |
| `summarize` | Browser → Lupus | Read and summarize a page |
| `index_page` | Browser → Lupus | Add a visited page to the local index |
| `get_status` | Browser → Lupus | Check daemon health, model status |
| `trust_update` | Lupus → Browser | Push updated trust score for current page |
| `index_stats` | Browser → Lupus | Get index size, last sync time |

### Error Format

```json
{
  "id": "req-001",
  "status": "error",
  "error": {
    "code": "model_not_loaded",
    "message": "Search model is still loading"
  }
}
```

---

## Lifecycle

### Startup

1. Lepus browser launches
2. Browser spawns Lupus daemon as a child process (or connects to existing)
3. Lupus loads security model first (needed for page load scanning)
4. Lupus loads search model base + default adapter
5. Lupus opens IPFS client connection to cooperative gateway
6. Lupus sends `ready` status to browser
7. Browser enables search and trust indicator UI

### Shutdown

1. Browser closing → sends `shutdown` to Lupus
2. Lupus saves index state to disk
3. Lupus closes IPFS connections
4. Lupus process exits

### Model Hot-Swap

```
Browser sends: {"method": "swap_adapter", "params": {"adapter": "content"}}
Lupus: unloads search adapter, loads content adapter (~100ms)
Lupus sends: {"status": "ok", "adapter": "content"}
```

---

## Daemon Components

### Agent (agent.rs)

Manages TinyAgent model lifecycle and inference:
- Load base model (GGUF via llama.cpp bindings)
- Load/swap LoRA adapters
- Process queries through the agent loop
- Tool calling: parse model output → execute tool → feed result back

### Security Scanner (security.rs)

Runs the code-trained security model:
- Accepts raw HTML + URL
- Returns trust score (0-100) + threat indicators
- Optimized for low latency (< 500ms per page)
- Loaded at startup, always in memory

### IPFS Client (ipfs.rs)

Lightweight IPFS via Iroh:
- Fetch content by CID
- Cache accessed content locally
- Publish index entries (opt-in)
- Connect to cooperative Pintheon gateway

### Crawler / Indexer (crawler.rs)

Builds and maintains the den (the local content store + search index):
- Indexes pages as user browses (via `index_page` calls from browser)
- Generates embeddings for semantic search
- Syncs den entries with cooperative (opt-in)
- Background periodic sync with cooperative index channel

### The Den (den.rs)

Lupus's local content store + semantic search index — named after the
wolf's den, where the pack stores what it brings home. See
`docs/LUPUS_TOOLS.md` §4.6 for the full data model.

- Stores document embeddings + metadata in `DenEntry` records
- Holds the local Iroh blob store backing each entry's `content_cid`
- Nearest-neighbor search for queries (semantic + keyword)
- Ranks by semantic similarity + CWP commitment signals
- Persisted to disk between sessions

Naming convention: "index" is the verb (the action of adding a page to
the den), "den" is the noun (the storage). IPC method names like
`index_page` and `index_stats` keep the verb form; internal Rust types
use the noun form (`Den`, `DenEntry`, `DenConfig`).

### Tools (tools/)

Each tool is a self-contained function the agent can call:

| Tool | Input | Output | Used By |
|------|-------|--------|---------|
| `search_subnet` | query string | matching datapod metadata | Search adapter |
| `search_local_index` | query embedding | ranked local results | Search adapter |
| `fetch_page` | URL (hvym or https) | page content | Search + Content adapters |
| `extract_content` | HTML | clean text, title, summary | Content adapter |
| `scan_security` | HTML + URL | trust score, threats | Security model |
| `crawl_index` | CID or URL | index entry | Crawler |

---

## Configuration

```yaml
# ~/.config/lupus/config.yaml

daemon:
  port: 9549
  host: "127.0.0.1"  # localhost only, never exposed

models:
  search_base: "~/.local/share/lupus/models/lupus-search-base.gguf"
  search_adapter: "~/.local/share/lupus/models/lupus-search-adapter.gguf"
  content_adapter: "~/.local/share/lupus/models/lupus-content-adapter.gguf"
  security: "~/.local/share/lupus/models/lupus-security.gguf"

ipfs:
  enabled: true
  gateway: "https://gateway.heavymeta.art"
  cache_dir: "~/.local/share/lupus/ipfs-cache/"
  max_cache_gb: 5

den:
  path: "~/.local/share/lupus/den/"
  max_entries: 100000
  contribution_mode: "off"  # off | anonymous | signed

cooperative:
  registry: "https://registry.heavymeta.art"
  contract_id: "CA2ACNHDRGFSFZYSPPZYE5MVZQBVMNH4HLCSQ43BPOWB4UIT2WK334DN"
```

---

## Resource Footprint

| Component | RAM | Disk | CPU |
|-----------|-----|------|-----|
| Search model (base + adapter) | ~1GB | ~750MB | Idle until query |
| Security model | ~500MB | ~500MB | Brief spike per page load |
| IPFS client (Iroh) | ~50MB | Cache varies | Minimal |
| Search index | ~100MB | ~500MB at 100K entries | Minimal |
| **Total daemon** | **~1.7GB** | **~2GB** | **Low average, spikes on queries** |

With the browser using ~500MB-1GB, total system usage is ~2.5-3GB — fits in 8GB RAM machines.
