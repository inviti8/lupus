# Lupus Training Runbook

**Self-contained, step-by-step guide** for training the Lupus security classifier on RunPod. Follow this top-to-bottom on your local workstation and your RunPod GPU instance. You should not need to flip back to other documentation while following this.

This runbook is for **v0.2** of the security model — the version with the balanced dataset (16K phishing + 16K malware + 16K safe), three classes (we dropped `suspicious`), capped class weights, gradient clipping, and the automated RunPod deploy tooling.

---

## What you're building

A fine-tuned **Qwen2.5-Coder-0.5B** that classifies URLs into three classes:

- `safe` — legitimate site
- `phishing` — credential-stealing or impersonation attack
- `malware` — malware distribution URL

This is **Stage 1**: trained on URL features only (no HTML body fetching). Stage 2 (URL + HTML) comes after this works.

**Expected duration:** ~30-60 minutes of GPU time on an RTX 4090. Most of the time goes to environment setup and the initial model download — actual training is fast on a 0.5B model with 48K balanced examples.

**Expected cost:** ~$0.20-0.40 of GPU time at the $0.20-0.30/hr spot price in `US-IL-1`.

---

## Prerequisites checklist

Before starting, confirm:

- [ ] **`.env` file at the repo root** with all required credentials filled in (see `.env.example`):
  - `S3_ENDPOINT_URL`, `S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_DEFAULT_REGION` (RunPod S3 storage)
  - `HF_TOKEN` (HuggingFace, for downloading the base model)
  - `WANDB_API_KEY`, `WANDB_PROJECT`, `WANDB_ENTITY` (Weights & Biases, for live training metrics)
  - `RUNPOD_API_KEY` (RunPod, full-access — used by `tools/runpod_deploy.py` to provision and manage pods)
- [ ] **Built dataset on disk** at `datasets/security/examples/train.jsonl` and `eval.jsonl`. If missing: `python datasets/security/build_dataset.py --balance --max-per-class 20000`
- [ ] **A RunPod network volume** in `US-IL-1`. The default volume id `7oqdtnkk5f` is wired into `tools/runpod_deploy.py`. If you have a different one, pass `--volume YOUR_VOLUME_ID`.
- [ ] **Python 3.10+** locally with deps: `pip install boto3 python-dotenv wandb requests huggingface_hub`

---

## Phase 1 — Local: push the dataset to S3

This runs **once** from your local Windows machine. Uploads the built train/eval JSONL files to your RunPod S3 bucket so the pod can pull them later.

### Step 1.1 — Verify the dataset exists locally

```bash
cd D:/repos/lupus
python datasets/security/schema.py
```

Expected output: `60000 examples validated  All entries valid.`

If the dataset doesn't exist or you want to rebuild from updated source data:

```bash
python datasets/security/fetch/openphish.py --force         # ~300 phishing URLs (no auth)
python datasets/security/fetch/phishing_database.py --force # ~789K phishing URLs (no auth, GitHub)
python datasets/security/fetch/urlhaus.py --force           # ~21K malware URLs
python datasets/security/fetch/tranco.py --top 50000 --force # 50K safe URLs

python datasets/security/build_dataset.py --balance --max-per-class 20000 --verbose
python datasets/security/schema.py
```

### Step 1.2 — Push to S3

```bash
python training/push_dataset.py
```

If files are already in S3 (which they probably are after the first run), it'll skip them. Use `--force` to overwrite.

**Expected output:**
```
INFO Uploading datasets/security/examples/train.jsonl (~17 MB) → s3://7oqdtnkk5f/datasets/security/train.jsonl
INFO Uploading datasets/security/examples/eval.jsonl (~5 MB) → s3://7oqdtnkk5f/datasets/security/eval.jsonl
INFO Uploading datasets/security/schema.py → s3://7oqdtnkk5f/datasets/security/schema.py
INFO Pushed: 3  Skipped: 0  Missing: 0
INFO Total uploaded: ~22 MB
```

### Step 1.3 — Verify

```bash
python training/s3_utils.py
```

You should see roughly:
```
Connected to s3://7oqdtnkk5f/
  3+ object(s) total
  - datasets/security/eval.jsonl
  - datasets/security/schema.py
  - datasets/security/train.jsonl
```

✓ **Phase 1 complete.** The dataset is in S3 and ready to be pulled by the pod.

---

## Phase 2 — Provision the RunPod instance (automated)

**Big change from earlier versions of this runbook:** instead of clicking through the RunPod web console (which is painful when capacity is tight), we use `tools/runpod_deploy.py` which polls the RunPod REST API and grabs an instance the moment one becomes available.

### Step 2.1 — Check current account state and GPU availability

```bash
python tools/runpod_status.py
```

This is read-only and free. It prints:
- Your account email and current spend rate
- Any active pods (should be 0 — terminate any leftovers first)
- Your network volumes
- Live GPU stock status in `US-IL-1`

Example output when nothing's available right now:
```
======================================================================
  Account
======================================================================
  Email:        you@example.com
  Spend rate:   $0.0010/hr
  Machine quota: 0

======================================================================
  Active pods
======================================================================
  No active pods.

======================================================================
  Network volumes
======================================================================
  - 7oqdtnkk5f
      name:        related_fuchsia_coyote
      size:        10 GB
      region:      US-IL-1

======================================================================
  GPU availability in US-IL-1
======================================================================
  NVIDIA GeForce RTX 4090                    Low             $0.200/hr  $0.340/hr
  ⚠ No GPUs currently available in US-IL-1
```

The "Low" stock value is normal in `US-IL-1`. Even at "Low" the deploy script can usually grab one within a few minutes of polling.

### Step 2.2 — Deploy the pod (with auto-retry)

**Dry run first** to verify the request body:

```bash
python tools/runpod_deploy.py --dry-run
```

Expected output:
```json
Deploy request:
{
  "name": "lupus-training-20260407-180000",
  "computeType": "GPU",
  "gpuTypeIds": ["NVIDIA GeForce RTX 4090"],
  "gpuCount": 1,
  "gpuTypePriority": "availability",
  "dataCenterIds": ["US-IL-1"],
  "dataCenterPriority": "availability",
  "interruptible": true,
  "imageName": "runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04",
  "containerDiskInGb": 20,
  "minVCPUPerGPU": 4,
  "minRAMPerGPU": 16,
  "ports": ["22/tcp", "8888/http"],
  "networkVolumeId": "7oqdtnkk5f",
  "volumeMountPath": "/workspace",
  "cloudType": "SECURE"
}
```

**If that looks right, deploy for real:**

```bash
python tools/runpod_deploy.py
```

The script will:
1. Submit the deploy request to RunPod's REST API
2. If RunPod responds with "no capacity available", wait 30 seconds and retry
3. Continue retrying for up to 30 minutes (60 attempts × 30s)
4. As soon as RunPod accepts the request and provisions a pod, print the pod ID and connection info

Expected success output:
```
[18:00:01] Attempt 1/60: deploying...
[18:00:02]   no capacity right now: HTTP 400: ...
[18:00:02]   waiting 30s before retry...
[18:00:32] Attempt 2/60: deploying...
[18:00:33] ✓ DEPLOYED: pod abc123def456

======================================================================
  Pod is up
======================================================================
  ID:           abc123def456
  Name:         lupus-training-20260407-180000
  Status:       RUNNING
  GPU:          NVIDIA GeForce RTX 4090
  Datacenter:   US-IL-1
  Cost/hr:      $0.200

  Web console:  https://www.runpod.io/console/pods

  Connect via web terminal:
    1. Open https://www.runpod.io/console/pods/abc123def456
    2. Click 'Connect' → 'Start Web Terminal'
```

### Step 2.3 — Common deploy options

```bash
# Override defaults
python tools/runpod_deploy.py --max-bid 0.40              # higher max bid
python tools/runpod_deploy.py --on-demand                 # use on-demand instead of spot ($0.34/hr in US-IL-1)
python tools/runpod_deploy.py --no-retry                  # one attempt only, fail fast
python tools/runpod_deploy.py --max-attempts 240          # poll for 2 hours instead of 30 min
python tools/runpod_deploy.py --gpu "NVIDIA RTX A6000" --gpu "NVIDIA GeForce RTX 4090"   # multiple acceptable GPUs
```

### Step 2.4 — Connect to the pod

The script prints the URL. Open it in your browser, click "Connect" → "Start Web Terminal".

You should see a shell prompt like `root@abc123def456:/#`. The current working directory will probably be `/` or `/root`.

✓ **Phase 2 complete** when you have a shell prompt on the new pod.

---

## Phase 3 — Bootstrap the pod

Everything from here happens **on the RunPod pod**, not your local machine.

### Step 3.1 — Install minimal tools you'll actually need

The RunPod PyTorch base image is minimal — no `nano`, no `tmux`, no `less`. Install these first; they'll save you pain later:

```bash
apt-get update && apt-get install -y nano tmux less
```

You're root in the container, so no `sudo` needed.

### Step 3.2 — Clone the repo into the network volume

The volume is mounted at `/workspace` and persists across pod terminations. Always clone the repo into it.

If `/workspace/lupus` already exists from a previous pod (and isn't damaged from earlier cleanup operations):

```bash
cd /workspace/lupus
git pull
```

Otherwise — and this is the safer default after any cleanup — start fresh:

```bash
cd /workspace
rm -rf lupus
git clone https://github.com/inviti8/lupus.git
cd lupus
```

### Step 3.3 — Paste your `.env` file (no editor needed)

The `.env` file is gitignored, so it's not in the cloned repo. Paste it via heredoc — no editor required:

```bash
cat > .env <<'EOF'
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

Replace each `<...>` with the actual value from your local `.env`. The single quotes around `'EOF'` prevent the shell from interpreting `$` characters in the values.

Verify (without printing the secrets):

```bash
sed 's/=.*/=<set>/' .env
```

You should see all the keys with `<set>` after each `=`.

### Step 3.4 — Run the bootstrap script inside tmux

**Always run inside tmux** so a closed browser tab doesn't kill your work:

```bash
tmux new -s lupus
```

You're now inside tmux (look for the green status bar at the bottom). Then:

```bash
bash training/setup_pod.sh
```

This installs Python deps, downloads the dataset from S3 (or uses existing volume copy), verifies GPU, and confirms wandb login.

**Expected output:**
```
============================================
  Lupus RunPod bootstrap
============================================
Repo root: /workspace/lupus
Network volume detected at /workspace (deps and HF cache will persist)
.env found
HuggingFace cache: /workspace/.cache/huggingface (persists on volume)
System packages already present (skipping apt-get)
Installing Python dependencies (this may take a few minutes the first time)...
Python deps installed

Checking GPU availability...
  PyTorch version:  2.4.0+cu124
  CUDA available:   True
  CUDA version:     12.4
  Device 0:         NVIDIA GeForce RTX 4090 (23 GB)

Pulling training dataset from S3...
Dataset downloaded
  train.jsonl: 48000 lines, 17.4 MB
  eval.jsonl: 12000 lines, 4.4 MB

Verifying Weights & Biases login...
  W&B login OK

============================================
  Pod is ready. To start training:
    python training/train_security.py
============================================
```

### Common issues at this step

| Symptom | Fix |
|---|---|
| `pip install` is slow | Normal. PyTorch is already in the base image; only the lighter deps need to install. ~30s-2min. |
| `CUDA available: False` | Wrong base image. Re-deploy with `runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04`. |
| `Dataset downloaded` shows 0 bytes | S3 credentials are wrong on the pod. Re-paste `.env`. |
| `wandb.AuthenticationError` | `WANDB_API_KEY` is wrong/missing. The training script can run without wandb if you pass `--no-wandb`. |
| `setup_pod.sh: Permission denied` | Use `bash training/setup_pod.sh` (the runbook command) instead of `./setup_pod.sh`. The committed file should have the executable bit set, but `bash` always works. |

✓ **Phase 3 complete.** The pod is fully provisioned.

---

## Phase 4 — Start training

Still inside your tmux session:

```bash
python training/train_security.py
```

**Default hyperparameters (v0.2):**

| Setting | Value | Why |
|---|---|---|
| Epochs | 3 | Plenty for a 0.5B model on 48K balanced examples |
| Batch size | 32 | Comfortable on 24 GB VRAM |
| Learning rate | 2e-5 | Lower than v0.1's 5e-5 — much more stable |
| Warmup ratio | 0.1 | Gentler initial ramp |
| Max gradient norm | 1.0 | Gradient clipping prevents the v0.1 explosions |
| Class weights | Capped at 10x | v0.1 used uncapped 60x weights and exploded |
| Mixed precision | bf16 | Fast on Ada Lovelace |
| Save every | 200 steps | ~30s on RTX 4090; checkpoints upload to S3 immediately |
| Eval every | 100 steps | Per-class metrics on 12K eval set |

**Expected output (early):**

```
INFO CUDA device: NVIDIA GeForce RTX 4090 (23 GB)
INFO Loading tokenizer: Qwen/Qwen2.5-Coder-0.5B
INFO Loading model with classification head: Qwen/Qwen2.5-Coder-0.5B
INFO Loaded 48000 examples from datasets/security/examples/train.jsonl
INFO Loaded 12000 examples from datasets/security/examples/eval.jsonl
INFO Training class distribution:
INFO   safe          16000  (33.33%)
INFO   phishing      16000  (33.33%)
INFO   malware       16000  (33.33%)
INFO Loss class weights: {'safe': 1.0, 'phishing': 1.0, 'malware': 1.0}
INFO Starting training...
{'loss': 1.084, 'learning_rate': 2.0e-06, 'epoch': 0.01}
{'loss': 0.823, 'learning_rate': 4.0e-06, 'epoch': 0.02}
{'loss': 0.567, ...}
```

**Watch for:**
- Loss decreasing **smoothly** (not oscillating wildly like v0.1)
- All three per-class F1 scores rising **together** (not just safe and malware)
- `f1_macro` climbing past 0.85 by mid-training
- `false_positive_rate` staying below 0.05

### Step 4.1 — Open the wandb dashboard

In a separate browser tab: https://wandb.ai/heavymeta/lupus

You should see a new run named `lupus-security-stage2`. Click into it. Watch the live metrics:

- **train/loss** — should drop steadily, not oscillate
- **eval/f1_macro** — should rise steadily, target > 0.85
- **eval/precision_phishing** + **eval/recall_phishing** — the most important per-class metrics
- **eval/false_positive_rate** — should stay near zero (target < 5%)
- **train/grad_norm** — should stay in 0.1-2.0 range (gradient clipping caps it at 1.0 anyway)

If wandb is showing nothing after a minute, training hasn't started yet — the model is still loading. Be patient.

✓ **Phase 4 complete** when you see the first eval step (~step 100) with sane metrics in wandb.

### Step 4.2 — Detach from tmux

Once training is going and metrics look good, detach from tmux so you can close the browser tab without killing the run:

Press **`Ctrl-B`** then **`D`** (release Ctrl-B first, then press D).

You'll see `[detached (from session lupus)]`. The training keeps running. The browser tab can now close safely.

To reattach later from any new web terminal:
```bash
tmux attach -t lupus
```

---

## Phase 5 — Monitor training

The training run will take ~30-60 minutes. **You can do other work while it runs.**

### What to check periodically

Every 5-10 minutes, glance at the wandb dashboard or `tmux attach -t lupus` to see live output:

- **Loss is going down** smoothly
- **F1 macro is climbing** — should reach > 0.7 within the first epoch, > 0.85 by end
- **All three per-class F1s are rising** — especially phishing
- **False positive rate is below 5%**
- **Gradient norms are bounded** (≤ 1.0 due to clipping)

### Checkpointing

The script saves a checkpoint every 200 steps and **immediately uploads it to S3**:

```
INFO Uploaded checkpoint-200 to S3
INFO Uploaded checkpoint-400 to S3
```

Verify in another shell on the pod (or on your local Windows):

```bash
python training/s3_utils.py | grep checkpoint
```

This means **even if the spot pod gets killed mid-training**, you only lose at most ~30 seconds of progress between checkpoints.

### Volume space concern

Your network volume is 10 GB. Each checkpoint is ~1 GB and we keep up to 3 (`save_total_limit=3` in the training script). Plus the HF model cache is ~1 GB. Total worst case: ~5 GB used, ~5 GB free. Should be fine but if you see "no space left on device" errors, free up space:

```bash
# On the pod, check usage:
df -h /workspace

# Manually trim old checkpoints if needed:
ls -la training/output/lupus-security/
rm -rf training/output/lupus-security/checkpoint-200  # keep newest only
```

✓ **Phase 5 ongoing** — training continues to completion.

---

## Phase 6 — Recover from interruption (if it happens)

Spot pods can be killed at any moment. If your pod disappears mid-training:

### Step 6.1 — Provision a new pod via runpod_deploy.py

```bash
# On your local Windows machine:
cd D:/repos/lupus
python tools/runpod_deploy.py
```

Same script, same network volume — when the new pod comes up, your repo, dataset, HF cache, and even local checkpoints are still on the volume (assuming the volume wasn't damaged by a cleanup script earlier).

### Step 6.2 — Bootstrap and resume

On the new pod:

```bash
cd /workspace
# If lupus/ is intact from before:
cd lupus
git pull
# Otherwise:
# rm -rf lupus && git clone https://github.com/inviti8/lupus.git && cd lupus

# .env may also need re-pasting if the volume was wiped — see Phase 3.3

apt-get update && apt-get install -y tmux nano less   # quick to re-install
tmux new -s lupus
bash training/setup_pod.sh
python training/train_security.py --resume
```

The `--resume` flag tells the training script to:

1. Look for the latest `checkpoint-N/` directory in S3
2. Download it to local disk
3. Resume training from exactly where the previous pod left off (same step count, same optimizer state, same scheduler state)

The wandb run will also resume in place — same run continues receiving metrics, so loss curves stay continuous.

✓ **Phase 6 complete** when training resumes successfully.

---

## Phase 7 — Training complete

When training finishes, you'll see:

```
INFO Running final evaluation...
INFO Final metrics:
INFO   accuracy                       0.9650
INFO   eval_loss                      0.123
INFO   f1_macro                       0.940
INFO   f1_phishing                    0.920
INFO   f1_malware                     0.965
INFO   f1_safe                        0.935
INFO   false_positive_rate            0.019
INFO   precision_phishing             0.910
INFO   recall_phishing                0.930
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
INFO Found 8 files to download (~1 GB total)
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
    "https://roblox.com.ge/users/1654861376/profile",
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
    → phishing (92% confident)
  http://221.15.91.18:49869/i
    → malware (96% confident)
  https://github.com/inviti8/lupus
    → safe (97% confident)
  https://roblox.com.ge/users/1654861376/profile
    → phishing (88% confident)   ← TLD lookalike attack
```

✓ **Phase 7 complete.** You have a trained model that classifies URLs.

---

## Phase 8 — Shut down the pod

**Don't forget to terminate the pod** when training is done. Spot instances bill by the second, but they keep billing as long as the pod exists, even idle.

### Option A — Use the RunPod console
```
https://www.runpod.io/console/pods → your pod → ⋮ → Terminate
```
**Important:** "Terminate" (destructive, stops billing) NOT "Stop" (preserves disk, still partially billed).

### Option B — Use the API
You can also terminate via the REST API. Quick one-liner:
```bash
python -c "
import os, requests
from dotenv import load_dotenv
load_dotenv()
pod_id = 'YOUR_POD_ID'
r = requests.delete(
    f'https://rest.runpod.io/v1/pods/{pod_id}',
    headers={'Authorization': f'Bearer {os.environ[\"RUNPOD_API_KEY\"]}'},
)
print(r.status_code, r.text)
"
```

### Verify the pod is gone
```bash
python tools/runpod_status.py
```
Should show "No active pods."

The network volume **stays** — that's intentional, it's where your dataset, base model cache, and final model live for the next session.

---

## Troubleshooting

### "CUDA out of memory"
Reduce `--batch-size`:
```bash
python training/train_security.py --batch-size 16
```
Qwen2.5-Coder-0.5B should easily fit in 24 GB at batch size 32, but if you somehow got a smaller GPU, halve the batch size.

### Loss is NaN
Lower the learning rate:
```bash
python training/train_security.py --learning-rate 1e-5
```

### F1 phishing stays low
With v0.2's balanced dataset this shouldn't happen. If it does:
1. Verify the dataset has equal classes: `python datasets/security/schema.py` plus a manual count
2. Check the loss curve — if it's oscillating, lower the learning rate
3. Try more epochs: `python training/train_security.py --epochs 5`

### Pod can't be deployed (no capacity for hours)
Try escalating in this order:
1. Increase max bid: `python tools/runpod_deploy.py --max-bid 0.40`
2. Switch to on-demand: `python tools/runpod_deploy.py --on-demand` (~$0.34/hr in US-IL-1)
3. Move to a different region: create a new network volume in `us-east` or `eu-central`, update `.env` with the new endpoint, re-push the dataset, and re-run deploy with `--region US-EAST-1` (or whichever)

### "No space left on device" during training
Your network volume is full. Free space:
```bash
# Delete old checkpoints, keeping only the latest
ls training/output/lupus-security/
rm -rf training/output/lupus-security/checkpoint-200 training/output/lupus-security/checkpoint-400
# Or trim the HF cache (will re-download next run):
rm -rf /workspace/.cache/huggingface
```
For a longer-term fix, resize the volume in the RunPod console (Storage → your volume → Edit → Increase size).

### `Dataset files not found` on the pod
The `setup_pod.sh` script didn't complete the S3 download. Check S3 credentials in `.env` and re-run `bash training/setup_pod.sh`.

### `pip install` hangs
RunPod's network is occasionally slow. Cancel with Ctrl-C and retry. If consistently failing:
```bash
pip install -r training/requirements.txt --index-url https://pypi.org/simple
```

### Browser terminal closes mid-training
You forgot to use tmux. The training is dead. Re-deploy a new pod (`python tools/runpod_deploy.py`) and `--resume` from the latest S3 checkpoint.

**Always run training inside tmux.** Phase 3.4 and Phase 4.2 both emphasize this.

### The wandb dashboard shows nothing
The first metrics appear at step `--logging-steps` (default 20). If you see nothing after several minutes, training hasn't actually started — the model is still loading. Watch the pod's tmux output for `Starting training...`.

### Volume cleanup damaged the cloned repo
If a cleanup operation deleted files from `/workspace/lupus`, just `rm -rf lupus && git clone` again. The volume's persistent state is the dataset (S3) and HF model cache — both of which auto-recover. The repo itself comes from GitHub.

---

## Reference: command quick-cheatsheet

```bash
# === LOCAL (your Windows workstation) ===

# Check RunPod state and GPU availability
python tools/runpod_status.py

# Deploy a pod (auto-retries until capacity available)
python tools/runpod_deploy.py
python tools/runpod_deploy.py --dry-run             # preview without deploying
python tools/runpod_deploy.py --on-demand           # use on-demand instead of spot
python tools/runpod_deploy.py --max-attempts 240    # poll for 2 hours

# Build dataset (only if rebuilding from updated source data)
python datasets/security/build_dataset.py --balance --max-per-class 20000

# Push dataset to S3
python training/push_dataset.py
python training/push_dataset.py --force             # overwrite existing

# Verify S3 contents
python training/s3_utils.py

# Pull trained model after training
python training/pull_model.py --model security


# === POD (RunPod GPU instance) ===

# First-time bootstrap
apt-get update && apt-get install -y nano tmux less
cd /workspace
rm -rf lupus
git clone https://github.com/inviti8/lupus.git
cd lupus
# (paste .env via heredoc)
tmux new -s lupus
bash training/setup_pod.sh

# Start training
python training/train_security.py

# Detach: Ctrl-B then D

# Reattach later
tmux attach -t lupus

# Resume after interruption
python training/train_security.py --resume

# Custom hyperparameters
python training/train_security.py --epochs 5 --batch-size 64 --learning-rate 1e-5

# Run without wandb / S3 (debugging)
python training/train_security.py --no-wandb
python training/train_security.py --no-s3
```

---

## What's next after this works

- **Iterate**: based on the eval metrics, adjust hyperparameters and re-run. With `tools/runpod_deploy.py` automating provisioning, the iteration loop is fast.
- **Stage 2**: add HTML body content to the training (requires running `html_fetcher.py` on Tranco URLs first). The model becomes much more capable when it can read page structure.
- **Search adapter**: similar pipeline but with TinyAgent-1.1B base, LoRA adapters, and the `knowledge_aware.jsonl` we built from the folklore compendium.
- **Export to GGUF**: convert the final model to the format the daemon's llama.cpp integration expects.
- **Integration test**: wire the trained model into `daemon/src/security.rs` and test against the daemon's heuristic baseline.

These are separate next sessions. For now, focus on getting one v0.2 training run end-to-end.
