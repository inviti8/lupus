# Lupus Training Runbook

**Self-contained, step-by-step guide** for training the Lupus security classifier on RunPod. Follow this top-to-bottom on your local workstation and your RunPod GPU instance. You should not need to flip back to other documentation while following this.

---

## What you're building

A fine-tuned **Qwen2.5-Coder-0.5B** that classifies URLs into four classes:

- `safe` — legitimate site
- `phishing` — credential-stealing or impersonation attack
- `malware` — malware distribution URL
- `suspicious` — possibly malicious, low confidence

This is **Stage 1**: trained on URL features only (no HTML body fetching). It is the simpler, lower-risk first iteration. Stage 2 (URL + HTML) comes after this works.

**Expected duration:** 30 minutes to 2 hours of GPU time on an RTX 4090, depending on epochs and batch size. The actual training is fast — most of the time goes to environment setup and initial model download.

**Expected cost:** ~$0.15 to $0.60 of GPU time at the $0.29/hr spot price.

---

## Prerequisites checklist

Before starting, confirm:

- [ ] **`.env` file at the repo root** with valid credentials (see `.env.example`). The smoke test we ran earlier should have passed all four checks.
- [ ] **Built dataset on disk** at `datasets/security/examples/train.jsonl` and `eval.jsonl`. Run `python datasets/security/build_dataset.py` if not.
- [ ] **RunPod account with billing set up** and access to RTX 4090 or comparable GPU.
- [ ] **Python 3.10+** on your local workstation with `boto3`, `python-dotenv`, and `wandb` installed.
- [ ] **Git** access to push the lupus repo (optional but easier than uploading the code another way).

---

## Phase 1 — Local: push the dataset to S3

This runs **once** from your local Windows machine. You're uploading the built train/eval JSONL files to your RunPod S3 bucket so the pod can pull them later.

### Step 1.1 — Verify the dataset exists locally

```bash
cd D:/repos/lupus
ls -la datasets/security/examples/
```

You should see at least `train.jsonl` and `eval.jsonl`. They should total ~25 MB.

If not, build them first:

```bash
python datasets/security/build_dataset.py --verbose
```

### Step 1.2 — Push to S3

```bash
python training/push_dataset.py --verbose
```

**Expected output:**
```
INFO Uploading datasets/security/examples/train.jsonl (~19000 KB) → s3://7oqdtnkk5f/datasets/security/train.jsonl
INFO Uploading datasets/security/examples/eval.jsonl (~4800 KB) → s3://7oqdtnkk5f/datasets/security/eval.jsonl
INFO Uploading datasets/security/schema.py (~5 KB) → s3://7oqdtnkk5f/datasets/security/schema.py
INFO ---
INFO Pushed: 3  Skipped: 0  Missing: 0
INFO Total uploaded: 23.50 MB
```

If you re-run without `--force`, it skips already-uploaded files.

### Step 1.3 — Verify

```bash
python training/s3_utils.py
```

You should see:
```
Connected to s3://7oqdtnkk5f/
  3 object(s) total
  - datasets/security/eval.jsonl (4800.0 KB)
  - datasets/security/schema.py (5.0 KB)
  - datasets/security/train.jsonl (19000.0 KB)
```

✓ **Phase 1 complete.** The dataset is in S3 and ready to be pulled by the pod.

---

## Phase 2 — Provision the RunPod instance

This is a manual step in the RunPod web console. There is no button literally labeled "Provision" — the deploy button lives in the Pods section.

### Step 2.0 — A note on network volumes (read this first)

If you've already created a **network volume** in RunPod (recommended for spot training), the workflow is significantly nicer:

- The same network volume that exposes the S3 API at `s3api-us-il-1.runpod.io` (which our `.env` credentials talk to) **also mounts as a regular filesystem on any pod that attaches it**, typically at `/workspace`.
- This means: data we pushed to S3 is **already on the pod's disk** the moment the pod starts. No re-download needed.
- It also means: the HuggingFace base model cache (~1 GB), Python deps, training checkpoints, and the cloned repo can all live on the volume and **persist across pod terminations**. When a spot pod gets killed and replaced, the next pod starts where the last one left off in seconds, not minutes.

**Critical constraint:** **Network volumes are region-locked.** You can only attach a network volume to a pod that lives in the **same datacenter**. Your volume is in `us-il-1` (Israel), so you must filter for pods in that region. If `us-il-1` has no RTX 4090s available right now, options are:
1. Wait for capacity in `us-il-1`
2. Try a different GPU type that's available in `us-il-1` (RTX A5000, A4000)
3. Create a new network volume in another region (`us-east`, `eu-central`, etc.) and re-push the dataset to that volume's S3 endpoint by updating `.env`

### Step 2.1 — Find the deploy button (no "Provision" label exists)

1. Log in to https://www.runpod.io/console
2. In the **left sidebar**, click **Pods** (or **GPU Cloud** in some UI versions). NOT "Storage" — that's where your volume lives.
3. On the Pods page, look for one of these buttons (label varies by current UI version):
   - **`+ Deploy`** ← most common label
   - **`Deploy`**
   - **`+ GPU Pod`**
   - **`+ New Pod`**
   
   It's typically near the top of the page. If you can't find it, the alternative URL is https://www.runpod.io/console/deploy

### Step 2.2 — Configure the pod

On the deploy page:

| Setting | What to choose | Why |
|---|---|---|
| **GPU Type** | RTX 4090 (24 GB) | Plenty of VRAM for Qwen2.5-Coder-0.5B at batch size 32+ |
| **Pod Type** | Spot / Community Cloud / Interruptable (≈$0.29/hr) | Cheapest tier; spot interruption is recoverable thanks to S3 checkpointing |
| **Datacenter / Region** | **MUST match your network volume's region** (e.g. `us-il-1`) | Network volumes are region-locked |
| **Template / Image** | `runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04` | PyTorch + CUDA pre-installed; saves ~5 min on bootstrap |
| **Container Disk** | 20 GB | Pod-local ephemeral scratch space |
| **Volume Mount Path** | `/workspace` (default) | Where the network volume gets mounted inside the pod |
| **Network Volume** | **← attach your existing volume** (dropdown on the deploy page) | Persistence across pod restarts |

Then click **Deploy On-Demand** or **Deploy Spot** at the bottom.

Wait for the pod to come up (usually 30-60 seconds). The status in the Pods list will go from `Provisioning` → `Running`.

### Step 2.3 — Connect to the pod

Click the running pod in the Pods list. Then either:

- **Web terminal** (fastest): "Connect" button → "Start Web Terminal". Works in your browser, no SSH setup needed.
- **SSH** (better for long sessions): "Connect" → "SSH Terminal", copy the displayed `ssh root@<host> -p <port>` command and run it locally.

Once connected, you should be in a shell on the pod. The current working directory will probably be `/` or `/root` — that's fine, we'll `cd` into the volume next.

✓ **Phase 2 complete** when you have a shell prompt on the pod.

---

## Phase 3 — Bootstrap the pod

Everything from here happens **on the RunPod pod**, not your local machine.

### Step 3.1 — Clone the repo (into the network volume!)

If you attached a network volume in Phase 2, **clone the repo into the volume mount**, not the pod's ephemeral filesystem. That way the cloned working tree, your `.env` file, and any installed Python deps survive pod termination.

```bash
cd /workspace                                  # ← the network volume mount
git clone https://github.com/inviti8/lupus.git
cd lupus
```

If `/workspace/lupus` already exists from a previous pod, just `cd` into it and pull:

```bash
cd /workspace/lupus
git pull
```

If the repo is private and `git clone` fails: use a personal access token (`https://<user>:<token>@github.com/inviti8/lupus.git`), or push your local working copy to a temporary remote, or use `runpodctl send` to copy the working directory from your local machine.

### Step 3.2 — Copy your `.env` file to the pod

The `.env` file is gitignored, so it's not in the cloned repo. You need to get it onto the pod.

**Easiest method — paste it via the web terminal:**

```bash
cat > .env  <<'EOF'
S3_ENDPOINT_URL=https://s3api-us-il-1.runpod.io
S3_BUCKET=7oqdtnkk5f
AWS_ACCESS_KEY_ID=<your access key>
AWS_SECRET_ACCESS_KEY=<your secret key>
AWS_DEFAULT_REGION=us-il-1
HF_TOKEN=<your hf token>
WANDB_API_KEY=<your wandb key>
WANDB_PROJECT=lupus
WANDB_ENTITY=heavymeta
EOF
```

Replace each `<...>` with the actual value from your local `.env`. Then verify:

```bash
ls -la .env
cat .env | grep -v SECRET | grep -v KEY  # show non-secret values
```

**Alternative — use `runpodctl send`** (from your local machine):

```bash
# On local: install runpodctl first if needed: https://github.com/runpod/runpodctl
runpodctl send .env
# It will print a code. On the pod, run:
# runpodctl receive <code>
```

### Step 3.3 — Run the bootstrap script

```bash
bash training/setup_pod.sh
```

This installs Python deps, downloads the dataset from S3, and verifies GPU access. Expected output:

```
============================================
  Lupus RunPod bootstrap
============================================
Repo root: /workspace/lupus
.env found
Installing system packages (apt-get)...
Installing Python dependencies (this may take a few minutes)...
Python deps installed

Checking GPU availability...
  PyTorch version:  2.4.0+cu124
  CUDA available:   True
  CUDA version:     12.4
  Device count:     1
  Device 0:         NVIDIA GeForce RTX 4090 (23 GB)

Pulling training dataset from S3...
INFO Downloading s3://7oqdtnkk5f/datasets/security/train.jsonl → datasets/security/examples/train.jsonl
INFO Downloading s3://7oqdtnkk5f/datasets/security/eval.jsonl → datasets/security/examples/eval.jsonl
Dataset downloaded
  train.jsonl: 57372 lines, 19.4 MB
  eval.jsonl: 14342 lines, 4.9 MB

Verifying Weights & Biases login...
  W&B login OK

============================================
  Pod is ready. To start training:
    python training/train_security.py
```

**Common issues at this step:**

- `pip install` is slow → that's normal, the deps total ~5 GB downloaded. Be patient (3-10 min).
- `CUDA available: False` → wrong base image. Recreate the pod with a CUDA-enabled PyTorch image.
- `Dataset downloaded` but the file sizes are 0 → S3 credentials are wrong on the pod. Re-paste the `.env`.
- `wandb.errors.AuthenticationError` → WANDB_API_KEY is wrong or missing in `.env`. The training script can run without wandb if you pass `--no-wandb`.

✓ **Phase 3 complete.** The pod is fully provisioned.

---

## Phase 4 — Start training

```bash
python training/train_security.py
```

**Default hyperparameters** (suitable for the first run):

- 3 epochs
- Batch size 32
- Learning rate 5e-5
- Save checkpoint every 200 steps (~30 sec on RTX 4090)
- Eval every 100 steps
- Per-step logging every 20 steps
- bf16 mixed precision (faster on Ada Lovelace GPUs)
- Class-weighted CrossEntropy loss (compensates for the 300:21K:50K imbalance)

**Expected output (early):**

```
INFO CUDA device: NVIDIA GeForce RTX 4090 (23 GB)
INFO Loading tokenizer: Qwen/Qwen2.5-Coder-0.5B
INFO Loading model with classification head: Qwen/Qwen2.5-Coder-0.5B
INFO Loaded 57372 examples from datasets/security/examples/train.jsonl
INFO Loaded 14342 examples from datasets/security/examples/eval.jsonl
INFO Training class distribution:
INFO   safe          40000  (69.72%)
INFO   phishing        240  ( 0.42%)
INFO   malware       17132  (29.86%)
INFO   suspicious        0  ( 0.00%)
INFO Loss class weights: {'safe': 0.36, 'phishing': 59.76, 'malware': 0.84, 'suspicious': 0.0}
INFO Starting training...
{'loss': 1.234, 'learning_rate': 4.9e-05, 'epoch': 0.01}
{'loss': 0.987, 'learning_rate': 4.8e-05, 'epoch': 0.02}
...
```

**Watch for:**

- Loss decreasing from initial value (~1.2) toward 0.1-0.3
- `f1_macro` rising in eval steps (target: > 0.85 by end of training)
- `false_positive_rate` staying low (target: < 0.05)
- Per-class `f1_phishing` rising — this is the hardest class because of small sample count

### Step 4.1 — Open the wandb dashboard

In a separate browser tab, go to:

```
https://wandb.ai/heavymeta/lupus
```

You should see a new run named `lupus-security-stage1`. Click into it. Watch the live metrics:

- **train/loss** — should drop steadily
- **eval/f1_macro** — should rise steadily
- **eval/precision_phishing**, **eval/recall_phishing** — the most important per-class metrics
- **eval/false_positive_rate** — should stay near zero (target < 5%)

If wandb is showing nothing after a minute, training hasn't started yet — the model is still loading. Be patient.

✓ **Phase 4 complete** when you see the first eval step (~step 100) with metrics in wandb.

---

## Phase 5 — Monitor training

The training run will take 30 minutes to 2 hours depending on hyperparameters. **You can do other work while it runs.**

### What to check periodically

Every 5-10 minutes, glance at the wandb dashboard or `tail` the pod's terminal output:

- **Loss is going down** — if it plateaus immediately or oscillates wildly, something is wrong (probably learning rate too high)
- **F1 macro is going up** — should reach > 0.7 within the first epoch, > 0.85 by end
- **False positive rate is low** — < 5%, ideally < 2%
- **Phishing recall is improving** — this is the hardest metric because phishing is the smallest class. If it stays at 0, the class weighting isn't working

### Checkpointing

The script saves a checkpoint every 200 steps and **immediately uploads it to S3**:

```
INFO Uploaded checkpoint-200 to S3
INFO Uploaded checkpoint-400 to S3
...
```

You can verify in another shell:

```bash
python training/s3_utils.py | grep checkpoint
```

This means **even if the spot pod gets killed mid-training**, you only lose at most ~30 seconds of progress between checkpoints.

✓ **Phase 5 ongoing** — training continues to completion.

---

## Phase 6 — Recover from interruption (if it happens)

Spot instances can be killed at any moment. If your pod disappears mid-training, here's how to resume:

### Step 6.1 — Provision a new pod (same as Phase 2)

Same GPU type, same image, same disk size.

### Step 6.2 — Bootstrap and resume

```bash
cd /workspace
git clone https://github.com/inviti8/lupus.git
cd lupus
# Re-paste .env (Phase 3.2)
bash training/setup_pod.sh
python training/train_security.py --resume
```

The `--resume` flag tells the training script to:

1. Look for the latest `checkpoint-N/` directory in S3
2. Download it to local disk
3. Resume training from exactly where the previous pod left off (same step count, same optimizer state, same scheduler state)

**Expected output:**

```
INFO Pulling checkpoint from s3://.../models/lupus-security/checkpoints/checkpoint-1400
INFO Resuming from /workspace/lupus/training/output/lupus-security/checkpoint-1400
INFO Starting training...
{'loss': 0.234, 'learning_rate': 1.5e-05, 'epoch': 1.45}
```

The wandb run will also resume in place — the same run will continue receiving metrics, so your loss curves stay continuous.

✓ **Phase 6 complete** when training resumes successfully.

---

## Phase 7 — Training complete

When training finishes, you'll see:

```
INFO Running final evaluation...
INFO Final metrics:
INFO   accuracy                       0.9650
INFO   eval_loss                      0.1234
INFO   f1_macro                       0.8920
INFO   f1_phishing                    0.7800
INFO   f1_malware                     0.9700
INFO   f1_safe                        0.9810
INFO   false_positive_rate            0.0190
INFO   precision_phishing             0.7500
INFO   recall_phishing                0.8120
INFO   ...
INFO Saving final model to /workspace/lupus/dist/lupus-security
INFO Uploading final model to S3...
INFO Uploaded 8 files (1.0 GB) from dist/lupus-security → s3://7oqdtnkk5f/models/lupus-security/final/
INFO Done.
```

The final model is now in S3 at `models/lupus-security/final/`.

### Step 7.1 — Pull the model to your local machine

Back on your **local workstation**:

```bash
cd D:/repos/lupus
python training/pull_model.py --model security
```

**Expected output:**
```
INFO Found 8 files to download (1024.0 MB total)
INFO Downloading s3://7oqdtnkk5f/models/lupus-security/final/config.json → ...
INFO Downloading s3://7oqdtnkk5f/models/lupus-security/final/model.safetensors → ...
INFO Done. Model is at: D:/repos/lupus/dist/lupus-security
```

The model is now in `dist/lupus-security/` ready for inference testing or GGUF export.

### Step 7.2 — Quick inference smoke test (optional)

On your local machine (or on the pod), verify the model loads and makes sane predictions:

```python
from transformers import AutoTokenizer, AutoModelForSequenceClassification
import torch

model_path = "dist/lupus-security"
tokenizer = AutoTokenizer.from_pretrained(model_path)
model = AutoModelForSequenceClassification.from_pretrained(model_path)
model.eval()

test_urls = [
    "https://google.com/",
    "https://faceb00k-login.evil.com/verify",
    "http://221.15.91.18:49869/i",
    "https://github.com/inviti8/lupus",
]

for url in test_urls:
    inputs = tokenizer(f"URL: {url}", return_tensors="pt", truncation=True, max_length=128)
    with torch.no_grad():
        logits = model(**inputs).logits
    pred = logits.argmax(-1).item()
    label = model.config.id2label[pred]
    confidence = torch.softmax(logits, dim=-1)[0, pred].item()
    print(f"  {url}")
    print(f"    → {label} ({confidence:.0%} confident)")
```

You should see something like:

```
  https://google.com/
    → safe (98% confident)
  https://faceb00k-login.evil.com/verify
    → phishing (87% confident)
  http://221.15.91.18:49869/i
    → malware (95% confident)
  https://github.com/inviti8/lupus
    → safe (96% confident)
```

✓ **Phase 7 complete.** You have a trained model that classifies URLs.

---

## Phase 8 — Shut down the pod

Don't forget to **terminate the pod** in the RunPod console once training is done. Spot instances bill by the second, but they keep billing as long as the pod exists, even idle.

```
RunPod console → your pod → ⋮ → Terminate
```

(Or use `runpodctl stop` if you installed the CLI.)

---

## Troubleshooting

### "CUDA out of memory"
Reduce `--batch-size` to 16 or 8. Qwen2.5-Coder-0.5B should easily fit in 24 GB at batch size 32, but if it doesn't:
```bash
python training/train_security.py --batch-size 16
```

### Loss is NaN
Lower the learning rate:
```bash
python training/train_security.py --learning-rate 2e-5
```

### F1 phishing stays at 0
The class weights aren't strong enough or the model is collapsing to "always safe." Try lowering batch size (smaller batches mean phishing examples are over-represented per batch via the inverse-frequency weighting):
```bash
python training/train_security.py --batch-size 16 --learning-rate 2e-5
```

If this persists, you may need more phishing data — register for a free PhishTank API key and re-run the dataset build with `--api-key`, then re-push and re-train.

### Pod gets killed immediately every time
Spot capacity in your region is exhausted. Either:
1. Try a different region (RunPod has many)
2. Use Secure Cloud (on-demand) for ~50% more
3. Try a different GPU type (A4000, A5000)

### `Dataset files not found` on the pod
The `setup_pod.sh` script didn't complete the S3 download. Check S3 credentials in `.env` and re-run `bash training/setup_pod.sh`.

### Training is much slower than expected
- Verify GPU is being used: in another shell on the pod, run `nvidia-smi` and check that GPU utilization is > 50% during training.
- If GPU util is low, increase `--batch-size` or `--dataloader-num-workers`.
- If GPU util is high but it's still slow, you may be on a slower GPU than expected (3090 vs 4090) — check `nvidia-smi` for the actual model name.

### `pip install` hangs
RunPod's network is occasionally slow. Cancel with Ctrl-C and retry. If consistently failing, try `pip install -r training/requirements.txt --index-url https://pypi.org/simple` to bypass any mirror issues.

### The wandb dashboard shows nothing
The first metrics appear at step `--logging-steps` (default 20). If you see nothing after several minutes, training hasn't actually started — the model is still loading. Watch the pod's terminal for `Starting training...`.

---

## Reference: command quick-cheatsheet

```bash
# === LOCAL (your Windows workstation) ===

# Build dataset (only if not already built)
python datasets/security/build_dataset.py

# Push dataset to S3
python training/push_dataset.py

# Verify S3 contents
python training/s3_utils.py

# Pull trained model after training
python training/pull_model.py --model security

# === POD (RunPod GPU instance) ===

# Bootstrap fresh pod
git clone https://github.com/inviti8/lupus.git
cd lupus
# (paste .env)
bash training/setup_pod.sh

# Start training
python training/train_security.py

# Resume after interruption
python training/train_security.py --resume

# Custom hyperparameters
python training/train_security.py --epochs 5 --batch-size 64 --learning-rate 3e-5

# Run without wandb
python training/train_security.py --no-wandb

# Run without S3 (purely local, for debugging)
python training/train_security.py --no-s3
```

---

## What's next after this works

- **Iterate**: based on the eval metrics, adjust hyperparameters and re-run. The setup is fast enough that you can do 5-10 runs in an afternoon.
- **Stage 2**: add HTML body content to the training (requires running `html_fetcher.py` on Tranco URLs first).
- **Search adapter**: similar pipeline but with TinyAgent-1.1B base, LoRA adapters, and the `knowledge_aware.jsonl` we built from the folklore compendium.
- **Export to GGUF**: convert the final model to the format the daemon's llama.cpp integration expects.
- **Integration test**: wire the trained model into `daemon/src/security.rs` and test against the daemon's heuristic baseline.

These are separate next sessions. For now, focus on getting one training run end-to-end.
