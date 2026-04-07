# Security Model Training Dataset

Training data for the Lupus security scanner — a fine-tuned version of Qwen2.5-Coder-0.5B that classifies pages as safe / phishing / malware / suspicious and identifies threat indicators in HTML content.

## Strategy

Unlike the search adapter dataset (which requires Claude-generated training examples), the security dataset is built from **publicly available verified threat databases**. We don't generate this data — we download and process it.

| Source | Type | Volume | Cost | License |
|---|---|---|---|---|
| [OpenPhish](https://openphish.com/) | Phishing URLs (active feed) | ~500 active at any time | Free, **no registration** | Free non-commercial |
| [PhishTank](https://phishtank.org/) | Verified phishing URLs | 50,000+ active | Free but **requires API key registration** | CC-BY |
| [URLhaus](https://urlhaus.abuse.ch/) | Malware distribution URLs | 100,000+ active | Free, no registration | CC-0 |
| [Tranco Top 1M](https://tranco-list.eu/) | Legitimate sites (negative examples) | 1,000,000 ranked | Free, academic | Tranco license |
| Manual / heuristic | Known scam patterns, deceptive UI | ~100s | Volunteer | — |

**On PhishTank**: as of late 2025 PhishTank deprecated the anonymous CSV download endpoint and now requires a registered API key for bulk downloads. The registration is free but adds friction. OpenPhish provides a smaller no-registration alternative — use both together for diversity, or just OpenPhish if you want zero registration friction.

**Target:** ~20,000 labeled examples — 10K threat (split between phishing and malware) + 10K safe.

## Two-stage approach

**Stage 1 — URL features only (recommended first run).** Train on the URL string + lightweight metadata (domain, TLD, length, character entropy, etc.). The URL itself carries strong signal (lookalike domains, suspicious TLDs, encoded payloads, IDN homoglyph attacks). This is the fastest and lowest-risk path to a working model. No HTML fetching required, no risk of touching live phishing sites.

**Stage 2 — URL + HTML body.** Add fetched HTML content for richer feature learning. Phishing pages have characteristic structural markers (credential forms, brand impersonation, urgency language, obfuscated JavaScript). Qwen2.5-Coder is code-trained, so it natively understands HTML/JS structure.

Stage 2 is more powerful but introduces complexity (safe HTML fetching, content size budgets, the question of whether to ever touch live phishing sites). Build Stage 1 first, train it, evaluate it, then decide if Stage 2 is worth the effort.

## Critical safety considerations

**Never live-fetch HTML from phishing or malware URLs.** Many are still active and can:
- Serve drive-by exploits to the fetcher (mitigated by using `requests` not a browser, but not eliminated)
- Identify the fetcher's IP and add it to retaliation lists
- Update content based on fingerprinting (cloaking) so what we fetch isn't what users see

**The fetchers in this directory only live-fetch from Tranco (safe) URLs.** Phishing and malware URLs are processed for their URL features only. If HTML body is needed for those, use pre-archived datasets (Phish-IRIS, PhiUSIIL on UCI ML, Mendeley collections) — never fetch live.

**Tranco fetching has strict safety controls** even though the targets are legitimate: 10s timeout, 1MB max response, custom User-Agent identifying as research crawler, no JavaScript execution, no automatic redirect chains beyond 3, courteous rate limiting (1 req/sec).

## Directory structure

```
datasets/security/
  README.md                       (this file)
  schema.py                       Pydantic models + validation
  fetch/
    __init__.py
    openphish.py                  Download OpenPhish feed → raw/openphish.csv
    phishtank.py                  Download PhishTank CSV → raw/phishtank.csv (needs API key)
    urlhaus.py                    Download URLhaus CSV → raw/urlhaus.csv
    tranco.py                     Download Tranco list → raw/tranco.csv
    html_fetcher.py               Safe HTML retrieval utility
  build_dataset.py                Combine sources → examples/{train,eval}.jsonl
  raw/                            (gitignored) Downloaded source CSVs
  examples/
    train.jsonl                   ~16K labeled examples
    eval.jsonl                    ~4K held-out examples
```

## Schema

Every example matches `SecurityExample`:

```python
class SecurityExample(BaseModel):
    id: str                        # unique identifier
    source: Source                 # phishtank | urlhaus | tranco | manual
    source_id: Optional[str]       # original ID from the source database
    url: str                       # the URL itself
    domain: str                    # extracted hostname
    html_content: Optional[str]    # raw HTML, may be truncated; None for URL-only
    html_truncated: bool           # whether html was cut at the size limit
    label: Label                   # safe | phishing | malware | suspicious
    confidence: int                # 0-100, how confident we are in the label
    indicators: list[str]          # threat indicators detected
    target_brand: Optional[str]    # for phishing: what brand is impersonated
    threat_type: Optional[str]     # for malware: what kind
    fetched_at: str                # ISO timestamp
    verified: bool                 # whether the source verified this
```

## Training format

The dataset stores structured records. The training script transforms them into prompts at training time. Default prompt format:

```
### URL: https://faceb00k-login.evil.com/verify
### HTML: <html>...
### Analysis:
{"label": "phishing", "score": 95, "indicators": ["lookalike_domain", "credential_form"], "target_brand": "Facebook"}
```

The model learns to take URL + HTML and produce structured JSON matching the daemon's `ScanResponse` format (`daemon/src/protocol.rs`). This keeps training data and runtime output in sync.

## Build pipeline

```bash
# Install dependencies
pip install pydantic requests tldextract

# Step 1: Download source data (network-bound, ~5 minutes total)
python fetch/openphish.py                        # phishing (no registration)
python fetch/phishtank.py --api-key YOUR_KEY     # optional: more phishing data
python fetch/urlhaus.py                          # malware
python fetch/tranco.py --top 50000               # safe URLs (top 50K)

# Step 2: Optionally fetch HTML for safe Tranco URLs (network-bound, ~hours)
python fetch/html_fetcher.py --input raw/tranco.csv --output raw/tranco_html.jsonl

# Step 3: Build the combined dataset (fast, local)
python build_dataset.py --eval-split 0.2 --balance

# Step 4: Validate
python schema.py
```

## Cultural / ethical notes

The security dataset is technical, not cultural — there are no ethics issues comparable to the folklore compendium. The main ethical considerations are:
- **Don't retaliate against scrapers.** Our fetcher identifies itself and rate-limits.
- **Don't republish phishing content.** We store URL features and metadata, not page content beyond what training requires. The dataset is for training only, not redistribution.
- **Don't centralize threat intelligence.** This is research/training data, not an operational threat feed. For real-time blocking, use the upstream feeds directly.

## See also

- `daemon/src/security.rs` — the security scanner module that consumes the trained model
- `daemon/src/protocol.rs` — `ScanParams` and `ScanResponse` types
- `docs/TRAINING_STRATEGY.md` §2.3 — security dataset section of the training plan
- `base/config.yaml` — security model hyperparameters
