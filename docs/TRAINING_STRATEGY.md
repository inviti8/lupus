# Lupus Training Strategy

Complete plan for creating the first Lupus models from a blank slate. Includes base model selection, dataset creation, training methodology, infrastructure, and cost estimates.

---

## Starting Position

We start with:
- The Heavymeta Cooperative Bylaws (governance document)
- No existing training data
- No existing fine-tuned models
- No dedicated GPU hardware

Everything must be built: datasets curated, models selected, adapters trained, evaluation benchmarks created.

---

## Phase 1: Base Model Selection

### Search Model Base

The search model needs strong function-calling ability. Start from a **raw pretrained** base (not instruction-tuned) to minimize inherited bias.

| Candidate | Params | Why Consider | Bias Risk |
|-----------|--------|-------------|-----------|
| **TinyLlama-1.1B base** | 1.1B | Fast, well-understood architecture, large community | Low — minimal alignment in base |
| **TinyAgent-1.1B** | 1.1B | Purpose-built for function calling at the edge (Berkeley AI Research). ToolRAG for dynamic tool selection. Exceeds GPT-4-Turbo on tool-calling tasks (80% vs 79%). | Low — trained on tool-calling data, not conversational alignment |
| **Pythia-1.4B** (EleutherAI) | 1.4B | Fallback. Most transparent training data (The Pile). No corporate alignment. | Lowest — but needs function-calling training from scratch |

**Recommendation:** Start with **TinyAgent-1.1B**. It already understands tool calling — our LoRA adapters specialize it for Lepus-specific tools (search_subnet, fetch_page, extract_content) rather than teaching function calling from scratch. This is exactly the architecture TinyAgent was designed for. Fall back to Pythia only if TinyAgent carries unacceptable bias.

**Reference:** [TinyAgent: Function Calling at the Edge](https://arxiv.org/abs/2409.00608) (EMNLP 2024, [GitHub](https://github.com/SqueezeAILab/TinyAgent))

### Security Model Base

The security model needs code understanding. Use an **instruction-tuned code model** — bias in code models is less problematic because code analysis is more objective than natural language generation.

| Candidate | Params | Why Consider |
|-----------|--------|-------------|
| **Qwen2.5-Coder-0.5B** | 0.5B | Smallest viable code model, fast CPU inference |
| **Qwen2.5-Coder-1.5B** | 1.5B | Better accuracy, still fast |

**Recommendation:** Start with **Qwen2.5-Coder-0.5B** for speed. Security scanning is latency-sensitive (runs on every page load). Upgrade to 1.5B only if detection accuracy is insufficient.

### Estimated Cost: $0

Base models are open-source and free to download. No training required for base selection.

---

## Phase 2: Dataset Creation

This is the most labor-intensive phase. Quality training data determines model quality.

### 2.1 Search Adapter Dataset

The search adapter teaches the model to: understand queries, select tools, call functions, collate results.

**Dataset format:** TinyAgent-style function-calling examples.

```json
{
  "query": "digital art preservation techniques",
  "tools_available": ["search_subnet", "fetch_page", "extract_content"],
  "expected_calls": [
    {"tool": "search_subnet", "args": {"query": "digital art preservation"}},
    {"tool": "fetch_page", "args": {"url": "alice@articles/guide"}},
    {"tool": "extract_content", "args": {"format": "summary"}}
  ],
  "expected_output": "Found 3 relevant results on the HVYM subnet..."
}
```

**How to create this dataset:**

| Source | Method | Volume | Effort |
|--------|--------|--------|--------|
| Synthetic generation | Use a larger model (Claude/GPT-4) to generate query→tool-call pairs for cooperative-relevant topics | 5,000-10,000 examples | 1-2 weeks |
| Manual curation | Cooperative members write example queries and expected tool calls | 500-1,000 examples | 2-4 weeks |
| Bootstrapping from HVYM content | When cooperative has content, generate real query-result pairs from datapod metadata | 1,000-5,000 examples | Ongoing |

**Target dataset size:** 10,000 examples for initial training. Expand to 40,000 over time (matching TinyAgent's training set size).

### 2.2 Content Adapter Dataset

The content adapter teaches page reading and summarization.

```json
{
  "page_html": "<html>...(truncated)...</html>",
  "extracted_title": "Digital Art Preservation Guide",
  "extracted_summary": "Comprehensive guide covering IPFS storage, encryption...",
  "extracted_keywords": ["digital art", "IPFS", "preservation"],
  "content_type": "article"
}
```

**How to create:**

| Source | Method | Volume |
|--------|--------|--------|
| Common Crawl subset | Select high-quality pages, generate ground-truth extractions | 10,000 pages |
| HVYM member content | Real cooperative content with manual annotations | 500-2,000 pages |
| Wikipedia articles | Well-structured content for summarization training | 5,000 articles |

**Target:** 15,000 page-extraction pairs.

### 2.3 Security Training Dataset

The security model needs labeled examples of safe vs malicious pages.

```json
{
  "html": "<html>...<form action='https://evil.com/steal'>...</html>",
  "url": "https://faceb00k-login.com/verify",
  "label": "phishing",
  "score": 95,
  "indicators": ["credential_form", "lookalike_domain", "urgency_language"]
}
```

**Data sources (all publicly available):**

| Source | Type | Volume | Cost |
|--------|------|--------|------|
| [PhishTank](https://phishtank.org/) | Verified phishing URLs + pages | 50,000+ | Free |
| [OpenPhish](https://openphish.com/) | Phishing feed | 10,000+/month | Free tier |
| [URLhaus](https://urlhaus.abuse.ch/) | Malware distribution URLs | 100,000+ | Free |
| [Alexa/Tranco Top 1M](https://tranco-list.eu/) | Legitimate sites (negative examples) | 1,000,000 | Free |
| Manual collection | Scam pages, deceptive UI patterns | 1,000-5,000 | Manual effort |

**Target:** 20,000 labeled examples (10,000 phishing/malware, 10,000 safe).

### 2.4 Cooperative-Specific Data

From the Bylaws and cooperative documentation:

| Document | Use |
|----------|-----|
| Cooperative Bylaws | Teach model about cooperative governance, membership, terminology |
| HVYM documentation | Technical vocabulary, system architecture understanding |
| Pelt SVG schema | Understand pelt descriptions for search |
| NINJS metadata format | Parse and understand datapod content |

This is a small dataset (~100 documents) but important for cooperative-specific vocabulary.

### 2.5 Hare & Wolf Folklore — Knowledge-Aware Examples for the Search Adapter

Lupus and Lepus are named from the wolf and hare constellations. The project draws on Anishinaabe tradition (the wolf as Nanabozho's companion and pathfinder). To give the model genuine cultural depth — not just a mascot — we incorporate hare and wolf folklore from world mythology directly into the search adapter's training data, as **knowledge-aware function-calling examples**.

**Why this matters for a search engine:** When users search for topics the cooperative cares about — mythology, folklore, indigenous knowledge, storytelling — the model should have real understanding, not surface-level pattern matching. It also gives Lupus a distinct voice grounded in the traditions that inspired it.

**How it integrates with the search adapter:** The folklore data is not a separate dataset. It is a **subset of the search adapter training data** — roughly 10-15% of the 10K total examples, so ~1000-1500 knowledge-aware examples. Each one teaches the model to *both* demonstrate cultural knowledge *and* call the appropriate tools to search for related cooperative content. The model learns *both* to know the material *and* to search the local index for related entries. No separate adapter, no extra training cost — just a meaningful slice of the search dataset.

**Two artifacts, one source of truth:**

1. **`datasets/folklore/`** — The cultural compendium (`FolkloreTale` JSON entries). The curated, structured source material — the cultural artifact itself. Each entry includes the tale text, characters, themes, source citation, and cultural notes. Maintained as a permanent reference, valuable in its own right beyond ML training.

2. **`datasets/search/examples/knowledge_aware.jsonl`** — Training examples derived from the compendium. A conversion script generates one or more `SearchExample` entries from each `FolkloreTale`, embedding the knowledge into a query-response-tool-call training instance. Re-derivable from the compendium with different prompts as the training methodology evolves.

**Compendium coverage:**

| Tradition | Hare Stories | Wolf Stories | Sources |
|-----------|-------------|-------------|---------|
| **Anishinaabe / Ojibwe** | Nanabozho (Great Hare) as trickster-creator, culture hero | Ma'iingan (Wolf) as Nanabozho's companion, first to walk with humans, pathfinder | Basil Johnston, *Ojibway Heritage*; oral tradition collections |
| **Pan-Algonquian** | Michabo / Great Hare as creator figure across Algonquian peoples | Wolf clan traditions, wolf as teacher and scout | William Jones, *Ojibwa Texts*; Radin collections |
| **Japanese** | Moon rabbit (Tsuki no Usagi) — hare pounding mochi on the moon | Ōkami (great god/wolf) — Shinto wolf deity, protector of travelers | *Konjaku Monogatarishū*; Shinto shrine records |
| **Chinese** | Jade Rabbit (Yùtù) — companion of Chang'e on the moon, medicine maker | Wolf in Mongolian/Turkic origin myths (Ashina, Ergenekon) | *Chu Ci*; Mongolian *Secret History* |
| **Korean** | Moon rabbit (Dal-tokki) — rice cake maker on the moon | Founding myth of Dangun (bear and tiger, wolf variants) | Korean folk tale collections |
| **Mesoamerican** | Rabbit in the Moon (Aztec — Tecciztecatl's mark) | — | *Codex Chimalpopoca* |
| **West African / Yoruba** | Hare as trickster (Zomo, Kalulu, Sungura) across sub-Saharan traditions | Wolf-adjacent: hyena and jackal trickster tales | Harold Courlander, *A Treasury of African Folklore* |
| **European / Greco-Roman** | Lepus constellation myth; the hare in Aesop; Easter hare traditions | Romulus & Remus (wolf as mother/guardian); Fenrir (Norse); werewolf traditions | Ovid, *Metamorphoses*; Snorri Sturluson, *Prose Edda* |
| **Celtic / Irish** | Shape-shifting hare (Oisín, Cerridwen); hare as otherworld messenger | Wolves of Ossory (werewolves of Meath); Cormac mac Airt raised by wolves | Lady Gregory, *Gods and Fighting Men*; *Topographia Hibernica* |
| **Russian / Slavic** | Zaichik (little hare) in folk tales; Hare as Ivan's guide | Grey Wolf (Серый Волк) — magical helper, carries the hero, loyal companion | Afanasyev, *Russian Fairy Tales*; Baba Yaga cycles |
| **Indian / South Asian** | Jataka tales — the self-sacrificing hare (Śaśajātaka); Moon rabbit origin | Wolf in Panchatantra; Vrika in Vedic texts | *Jataka Tales*; *Panchatantra* |
| **Native American (Plains)** | Rabbit/Jackrabbit trickster stories (Lakota, Blackfoot) | Wolf as spirit guide, pack loyalty, pathfinder; Wolf clan across nations | Erdoes & Ortiz, *American Indian Myths and Legends* |
| **Arctic / Inuit** | Arctic hare in Inuit survival stories | Amarok (giant wolf) — solitary hunter, tests the worthy | Knud Rasmussen collections |
| **Egyptian** | Wenet (hare goddess, Hermopolis); hare hieroglyph (wn — "to be") | Wepwawet (opener of ways) — wolf/jackal war deity, pathfinder of the dead | *Pyramid Texts*; Hermopolis temple records |
| **Australian Aboriginal** | Hare-wallaby Dreaming stories | Dingo ancestor spirits (closest wolf analog) | Aboriginal oral tradition collections |
| **Aesop & fable traditions** | The Tortoise and the Hare; The Hare and the Hound | The Boy Who Cried Wolf; Wolf in Sheep's Clothing; The Wolf and the Crane | Aesop, Perry Index; La Fontaine |

**Key thematic threads across traditions:**

- **The Hare as trickster-creator** — Nanabozho, Zomo, Lepus: clever, fast, reshapes the world through wit
- **The Wolf as pathfinder-guardian** — Ma'iingan, Wepwawet, Grey Wolf: loyal, guides through danger, opens the way
- **The Wolf-Hare bond** — Nanabozho and Ma'iingan walk together; the wolf scouts ahead for the hare
- **Moon associations** — Jade Rabbit, Tsuki no Usagi, Śaśajātaka: the hare lives in the moon across cultures
- **Constellation myths** — Lupus and Lepus in the southern sky, adjacent constellations

**Compendium entry format (`FolkloreTale`):**

```json
{
  "id": "anishinaabe-nanabozho-and-maiingan",
  "tradition": "Anishinaabe",
  "title": "Nanabozho and Ma'iingan",
  "type": "creation_myth",
  "characters": [
    {"name": "Nanabozho", "role": "Great Hare, manitou, culture hero"},
    {"name": "Ma'iingan", "role": "Wolf, companion to Nanabozho, pathfinder"}
  ],
  "summary": "After creation, the Great Spirit gave Nanabozho a companion — the Wolf — and they walked the world together, naming all beings. Their fates remain forever linked.",
  "full_text": "...",
  "themes": ["companionship", "pathfinding", "naming", "linked fates"],
  "moral": "What befalls one of us befalls us all — the wolf's fate and the human's fate are bound together.",
  "source": {
    "citation": "Basil Johnston, Ojibway Heritage (1976)",
    "indigenous_author": true
  },
  "cultural_notes": "Basil Johnston was an Ojibwe scholar who explicitly published these stories to share Anishinaabe teachings with broader audiences.",
  "license": "indigenous_published"
}
```

**Derived training example (`SearchExample`):**

```json
{
  "id": "knowledge-anishinaabe-001",
  "category": "knowledge_aware",
  "user_query": "Tell me about the wolf in Anishinaabe tradition",
  "assistant_response": "In Anishinaabe tradition, the wolf — Ma'iingan — is the companion and pathfinder of Nanabozho, the Great Hare. After creation, the Creator gave Nanabozho the Wolf as a companion, and the two walked the world together, naming all beings. The teaching is that their fates are forever linked: 'what befalls one of us befalls us all.' This is from Basil Johnston's *Ojibway Heritage*.\n\n<|function_call|>{\"name\": \"search_local_index\", \"arguments\": {\"query\": \"Anishinaabe Nanabozho Maiingan wolf companion pathfinder\"}}<|end_function_call|>",
  "metadata": {
    "tradition": "Anishinaabe",
    "themes": ["companionship", "pathfinding", "linked fates"],
    "source_tale_id": "anishinaabe-nanabozho-and-maiingan"
  }
}
```

**How to build the compendium:**

| Source | Method | Volume | Cost |
|--------|--------|--------|------|
| Public domain texts | Gutenberg, Sacred Texts Archive, Internet Archive | 50-100 tales | $0 |
| Indigenous-authored collections | Basil Johnston, Erdoes & Ortiz, etc. (curated retellings) | 30-60 tales | Volunteer |
| Direct generation in Claude Code | Compendium entries written in-session, validated against Pydantic schemas | 100-150 tales | Subscription only |
| Academic folklore collections | Public domain pre-1928 sources | 30-50 tales | $0 |

**Realistic target:** 100-200 high-quality `FolkloreTale` entries in the compendium → 1000-1500 derived `SearchExample` entries (multiple training examples can be generated from each tale via different query angles). The earlier 500-1000 figure was overshooting — for a 1.1B model with LoRA, 100-200 culturally-resonant tales is genuinely sufficient.

**Important:** Prioritize Indigenous sources that are already public and shared by their communities. Do not scrape sacred or restricted knowledge. Use published collections by Indigenous authors where possible. When in doubt about a story's status, omit it.

### Estimated Cost: Phase 2

| Task | Method | Cost |
|------|--------|------|
| Synthetic data generation | API calls to Claude/GPT-4 for 10K examples | $50-200 |
| Phishing data collection | Free databases, scripted download | $0 |
| Safe page crawling | Scripted from Tranco list | $0 |
| Folklore compendium | Direct generation in Claude Code (subscription) | $0 |
| Manual curation | Cooperative member time | Volunteer hours |
| **Total** | | **$50-200** |

---

## Phase 3: Training Infrastructure

### Cloud GPU Options

| Provider | GPU | Price/Hour | Best For |
|----------|-----|-----------|----------|
| **RunPod** | RTX 4090 (interruptable/spot) | $0.29/hr | **Recommended** — dev, iteration, hyperparameter search. Requires checkpointing. |
| **RunPod** | RTX 4090 (on-demand) | $0.44/hr | Final publishable training runs (no interruption risk) |
| **RunPod** | A100 80GB | $1.64/hr | Overkill for 1B models with LoRA — only if doing larger experiments |
| **Lambda** | A100 80GB | $1.29/hr (reserved) | Cheapest A100 if committing hours |
| **Vast.ai** | A100 80GB | $0.80-1.50/hr (spot) | Alternative budget option |
| **Google Colab Pro** | A100 40GB | $10/month + compute | Good for experimentation |

**Spot/interruptable strategy:** Use $0.29/hr interruptable instances for everything except the final training run. Configure HuggingFace Trainer with `save_steps=200` (~5-10 min checkpoints) and use a RunPod network volume (~$0.07/GB/month) for persistence so a killed pod can be replaced and resumed without losing progress. Switch to on-demand only for the final publishable training run.

### Training Time Estimates

For QLoRA fine-tuning on a single RTX 4090 (24GB VRAM, spot pricing $0.29/hr):

| Model | Dataset Size | Epochs | Estimated Time | Cost |
|-------|-------------|--------|----------------|------|
| TinyAgent-1.1B search adapter | 10,000 examples | 3 | ~60 minutes | ~$0.30 |
| TinyAgent-1.1B content adapter | 15,000 examples | 3 | ~90 minutes | ~$0.45 |
| Qwen-Coder-0.5B security | 20,000 examples | 5 | ~40 minutes | ~$0.20 |
| **Total first training run** | | | **~3 hours** | **~$1** |

These estimates are for QLoRA (4-bit quantized base) with LoRA rank 16-32. The models are small enough that training is extremely fast even on consumer hardware.

### Iteration Budget

First-run training is cheap. The real cost is in **iteration** — training, evaluating, adjusting data, retraining:

| Phase | Runs | Hours | Cost (spot $0.29/hr) |
|-------|------|-------|---------------------|
| Initial training (3 models) | 1 | 3 hrs | $1 |
| Hyperparameter search | 10-20 | 15-30 hrs | $4-9 |
| Dataset quality iteration | 5-10 | 10-15 hrs | $3-5 |
| Evaluation and refinement | 10 | 10 hrs | $3 |
| Final training runs (on-demand $0.44/hr) | 3 | 5 hrs | $2 |
| **Total Phase 3** | | **45-65 hrs** | **$13-20** |

### Estimated Cost: Phase 3

**$13-20** for GPU compute on RTX 4090 spot instances. Far below the original $60-90 estimate that assumed A100 pricing.

---

## Phase 4: Training Execution

### 4.1 Search Adapter Training

```bash
# Download base model
python base/download_base.py --model pythia-1.4b

# Train search adapter
python adapters/search/train_search.py \
  --base-model pythia-1.4b \
  --dataset adapters/search/search_dataset/ \
  --output adapters/search/checkpoints/ \
  --lora-rank 16 \
  --epochs 3 \
  --batch-size 4 \
  --learning-rate 2e-4 \
  --quantize 4bit
```

**Key hyperparameters:**

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| LoRA rank | 16 | Good balance of capacity vs adapter size for 1B model |
| LoRA alpha | 32 | Standard 2x rank |
| Target modules | q_proj, v_proj, k_proj, o_proj | Attention layers for function-calling |
| Learning rate | 2e-4 | Standard for LoRA |
| Batch size | 4 | Fits in A100 with QLoRA |
| Epochs | 3 | Small dataset, avoid overfitting |
| Quantization | 4-bit NF4 | QLoRA for memory efficiency |

### 4.2 Content Adapter Training

Same base model, different dataset and potentially different target modules:

```bash
python adapters/content/train_content.py \
  --base-model pythia-1.4b \
  --dataset adapters/content/content_dataset/ \
  --output adapters/content/checkpoints/ \
  --lora-rank 32 \
  --epochs 3 \
  --learning-rate 1e-4
```

Higher LoRA rank (32) for content understanding — more capacity needed for summarization and extraction than for tool calling.

### 4.3 Security Model Training

Different base model, full fine-tune may be better than LoRA for 0.5B:

```bash
python security/train_security.py \
  --base-model qwen2.5-coder-0.5b \
  --dataset security/phishing_dataset/ \
  --safe-dataset security/safe_dataset/ \
  --output security/checkpoints/ \
  --epochs 5 \
  --learning-rate 5e-5 \
  --full-finetune  # Small enough for full fine-tune
```

At 0.5B parameters, full fine-tuning is feasible on a single GPU (~2GB model weights). This gives maximum accuracy for the security-critical task.

---

## Phase 5: Evaluation

### Search Adapter Benchmarks

| Benchmark | Metric | Target |
|-----------|--------|--------|
| Tool selection accuracy | % correct tool chosen for query | > 75% |
| Argument formatting | % correctly structured function calls | > 85% |
| Result relevance | Human-judged relevance of top-3 results | > 70% |
| Latency | Time to first tool call on CPU | < 2 seconds |

### Content Adapter Benchmarks

| Benchmark | Metric | Target |
|-----------|--------|--------|
| Title extraction | Exact match with ground truth | > 90% |
| Summary quality | ROUGE-L score | > 0.4 |
| Keyword extraction | F1 score | > 0.6 |
| Content classification | Accuracy (article/gallery/shop/etc.) | > 80% |

### Security Model Benchmarks

| Benchmark | Metric | Target |
|-----------|--------|--------|
| Phishing detection rate | True positive rate | > 90% |
| False positive rate | Safe pages incorrectly flagged | < 5% |
| Malware detection | True positive rate | > 85% |
| Latency | Time per page on CPU | < 500ms |

### Evaluation Dataset

Hold out 20% of each training dataset for evaluation. Additionally:
- 500 manually verified phishing pages (never seen in training)
- 500 manually verified safe pages from diverse categories
- 200 edge cases (legitimate pages that look suspicious)

---

## Phase 6: Export and Distribution

### GGUF Conversion

```bash
# Convert trained models to GGUF Q4_K_M for Lepus
python export/export_gguf.py \
  --model adapters/search/checkpoints/final/ \
  --base pythia-1.4b \
  --quantize Q4_K_M \
  --output dist/lupus-search-base.gguf

python export/export_gguf.py \
  --adapter adapters/search/checkpoints/final/adapter/ \
  --output dist/lupus-search-adapter.gguf

python export/export_gguf.py \
  --model security/checkpoints/final/ \
  --quantize Q4_K_M \
  --output dist/lupus-security.gguf
```

### Signing

```bash
# Sign with cooperative Ed25519 key
python export/sign_model.py \
  --model dist/lupus-search-base.gguf \
  --key $COOPERATIVE_SECRET_KEY \
  --output dist/lupus-search-base.gguf.sig
```

### Publishing

```bash
# Publish to cooperative registry
python export/publish.py \
  --models dist/ \
  --version 0.1.0 \
  --registry cooperative@models/lupus/
```

---

## Total Cost Estimate: First Lupus Model

| Phase | Cost | Time |
|-------|------|------|
| 1. Base model selection | $0 | 1 day (evaluation) |
| 2. Dataset creation | $50-200 | 2-4 weeks |
| 3. Training infrastructure | $13-20 | Included in phase 4 |
| 4. Training execution | Included above | 1-2 weeks (iterating) |
| 5. Evaluation | $0 (uses training GPU time) | 1 week |
| 6. Export and distribution | $0 | 1 day |
| **Total** | **$63-220** | **4-8 weeks** |

### Cost Breakdown

```
Dataset generation (synthetic via API, optional):  $50-200
Folklore compendium (in Claude Code session):      $0
GPU compute (45-65 hours RTX 4090 spot):           $13-20
                                                   --------
Total:                                             $63-220
```

This is remarkably cheap for a custom AI model. Key insights: at 0.5-1.5B parameters with LoRA, training is measured in minutes per run on consumer GPUs. Generating the folklore compendium directly in Claude Code (using a subscription) eliminates dataset generation cost for that portion. Spot RTX 4090 instances drop training compute by ~5x vs A100. The cost is dominated by optional iteration and any synthetic data generation through API calls — both of which can be reduced or eliminated.

### Cost Comparison

| Approach | Cost |
|----------|------|
| Fine-tune Lupus (0.5B + 1.4B) | $63-220 |
| Fine-tune a 7B model | $1,500-5,000 |
| Fine-tune a 70B model | $50,000-100,000 |
| Train a model from scratch | $1M+ |

---

## Timeline

| Week | Activity |
|------|----------|
| 1 | Download and evaluate base models. Set up training scripts. |
| 2-3 | Create datasets: synthetic generation, phishing data collection, page corpus, folklore compendium. |
| 4 | First training runs. Evaluate. Identify data quality issues. |
| 5 | Iterate: fix dataset problems, retrain, evaluate. |
| 6 | Hyperparameter search for best LoRA configuration. |
| 7 | Final training runs. Full evaluation against benchmarks. |
| 8 | Export to GGUF. Integration test with Lepus. Publish v0.1.0. |

---

## Tools and Dependencies

| Tool | Purpose |
|------|---------|
| [Hugging Face Transformers](https://github.com/huggingface/transformers) | Model loading and training |
| [PEFT](https://github.com/huggingface/peft) | LoRA/QLoRA adapter training |
| [bitsandbytes](https://github.com/bitsandbytes-foundation/bitsandbytes) | 4-bit quantization for QLoRA |
| [TRL](https://github.com/huggingface/trl) | Supervised fine-tuning trainer |
| [llama.cpp](https://github.com/ggerganov/llama.cpp) | GGUF conversion and local inference testing |
| [Weights & Biases](https://wandb.ai/) (optional) | Experiment tracking |

### Minimum Python Environment

```bash
pip install torch transformers peft bitsandbytes trl datasets accelerate
```

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Base model too biased | Search results reflect corporate alignment | Use Pythia (most transparent). Evaluate bias before training. |
| Insufficient training data | Poor search quality | Start with synthetic data, improve with real cooperative content over time |
| Security model false positives | Legitimate pages flagged | Extensive negative example training. Conservative threshold (warn, don't block). |
| Security model evasion | Phishing pages bypass detection | Continuous dataset updates from PhishTank/URLhaus. Community-reported misses. |
| CPU inference too slow | Poor user experience | Start with smallest models. Quantize aggressively. Profile and optimize. |
| Dataset quality issues | Model learns wrong patterns | Manual review of training examples. Hold-out evaluation. Human-in-the-loop iteration. |
