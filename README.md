# Lupus — The Pathfinder

Local AI agent daemon for the Lepus browser. Lupus runs as a standalone process — scouting ahead, reading the terrain, warning of dangers, and guiding the hare to what it seeks.

Named from the wolf constellation adjacent to Lepus in the night sky, and the Anishinaabe tradition of the wolf as Nanabozho's companion and pathfinder.

## Architecture

```
Lepus (browser)                    Lupus (daemon)
  │                                  │
  │  WebSocket (localhost:9549)      ├── TinyAgent search model + LoRA adapters
  │◄────────────────────────────────►├── Security model (code-trained)
  │                                  ├── IPFS client (Iroh)
  │  Browser does:                   ├── Distributed crawler/indexer
  │  - Render pages                  ├── Local semantic search index
  │  - Show results                  └── Tool implementations
  │  - Display trust scores
  │  - Pelt system
```

## Models

| Model | Base | Parameters | Role |
|-------|------|-----------|------|
| **Lupus Search** | TinyAgent-1.1B | ~1.1B + LoRA | Query routing, tool calling, page reading |
| **Lupus Security** | Qwen2.5-Coder-0.5B | ~0.5B | HTML/JS threat analysis, trust scoring |

## Principles

- **Local only** — all inference runs on the user's machine, no data leaves
- **Separate process** — doesn't bloat the browser, independent release cadence
- **Minimal bias** — cooperative controls all training data and alignment
- **Tools co-evolve with training** — tool implementations and training data in the same repo

## Structure

```
lupus/
  daemon/              ← Rust daemon process
    src/
      main.rs          ← WebSocket server, component lifecycle
      agent.rs         ← TinyAgent model + LoRA adapter management
      security.rs      ← Security model for HTML/JS scanning
      ipfs.rs          ← Iroh IPFS client
      crawler.rs       ← Distributed crawl + index
      index.rs         ← Local semantic search index
      tools/           ← Tool implementations the agent can call
  docs/
    TRAINING_STRATEGY.md   ← Full training plan and cost estimates
    DAEMON.md              ← IPC protocol, lifecycle, configuration
  base/                ← Base model selection and config
  adapters/            ← LoRA adapter training (Python)
  security/            ← Security model training (Python)
  export/              ← GGUF conversion, signing, publishing
  eval/                ← Evaluation benchmarks
```

## See Also

- [Lepus Browser](https://github.com/inviti8/lepus)
- [Training Strategy](docs/TRAINING_STRATEGY.md) — $110-290 total cost for first model
- [Daemon Architecture](docs/DAEMON.md) — IPC protocol, components, configuration
