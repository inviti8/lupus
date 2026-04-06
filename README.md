# Lupus — The Pathfinder

Local AI model for the Lepus browser. Lupus scouts ahead, reads the terrain, warns of dangers, and guides the hare to what it seeks.

Named from the wolf constellation adjacent to Lepus in the night sky, and the Anishinaabe tradition of the wolf as Nanabozho's companion and pathfinder.

## Models

| Model | Parameters | Role | Format |
|-------|-----------|------|--------|
| **Lupus Search** | ~1.1B + LoRA adapters | Query routing, tool calling, page reading | GGUF Q4 |
| **Lupus Security** | ~0.5B | HTML/JS threat analysis, trust scoring | GGUF Q4 |

## Principles

- **Local only** — all inference runs on the user's machine
- **Minimal bias** — base models are raw pretrained, cooperative controls all fine-tuning
- **Cooperative governed** — training data, alignment decisions, and model behavior are determined by Heavymeta members
- **Lightweight** — runs on CPU, ~1.5GB total, no GPU required

## See Also

- [Lepus Browser](https://github.com/inviti8/lepus) — the browser that runs Lupus
- [TRAINING_STRATEGY.md](docs/TRAINING_STRATEGY.md) — full training plan and cost estimates
