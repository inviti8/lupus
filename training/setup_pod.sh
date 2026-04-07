#!/usr/bin/env bash
# Lupus RunPod bootstrap script
#
# Run this on a fresh RunPod GPU instance after cloning the repo and
# placing your .env file at the repo root. It installs Python deps,
# pulls the dataset from S3 (or uses the network volume copy if
# already present), and verifies that the GPU is accessible.
#
# Recommended workflow with a network volume:
#     cd /workspace                                    # network volume mount
#     git clone https://github.com/inviti8/lupus.git
#     cd lupus
#     # Copy your .env file into place (paste, scp, or runpodctl)
#     bash training/setup_pod.sh
#
# Idempotent — safe to re-run. The script:
#   - Skips dataset re-download if files already exist on the volume
#   - Sets HF_HOME to /workspace/.cache/huggingface so the base model
#     persists across pod restarts (only if /workspace exists)

set -euo pipefail

echo "============================================"
echo "  Lupus RunPod bootstrap"
echo "============================================"

# 1. Locate the repo root (the parent of training/)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
echo "Repo root: $REPO_ROOT"

# 2. Detect whether we're on a persistent network volume
if [ -d "/workspace" ] && [ -w "/workspace" ]; then
    VOLUME_DETECTED=1
    echo "Network volume detected at /workspace (deps and HF cache will persist)"
else
    VOLUME_DETECTED=0
    echo "No network volume detected (state will be lost on pod termination)"
fi

# 3. Verify .env exists
if [ ! -f .env ]; then
    echo ""
    echo "ERROR: .env file not found at $REPO_ROOT/.env"
    echo ""
    echo "Copy your local .env file to this pod first. Options:"
    echo "  - paste it: cat > .env  (then paste, then Ctrl-D)"
    echo "  - scp it:   scp .env user@pod:$REPO_ROOT/"
    echo "  - runpodctl send: see RunPod docs"
    echo ""
    exit 1
fi
echo ".env found"

# 4. Set HuggingFace cache to the network volume so base models persist
#    across pod restarts (1 GB Qwen2.5-Coder doesn't need to be re-downloaded
#    every time a spot pod gets killed and replaced).
if [ $VOLUME_DETECTED -eq 1 ]; then
    export HF_HOME="/workspace/.cache/huggingface"
    mkdir -p "$HF_HOME"
    # Persist for future shells
    if ! grep -q "HF_HOME=" .env 2>/dev/null; then
        echo "" >> .env
        echo "# Auto-set by setup_pod.sh — HuggingFace cache on network volume" >> .env
        echo "HF_HOME=$HF_HOME" >> .env
    fi
    echo "HuggingFace cache: $HF_HOME (persists on volume)"
fi

# 5. System dependencies. RunPod's PyTorch base images already have all
#    of these (git, curl, wget, pip), so this step is usually a no-op.
#    Use sudo only if we're not root and sudo is available — RunPod
#    containers run as root with no sudo installed.
if [ "$EUID" -eq 0 ]; then
    SUDO=""
elif command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
else
    SUDO=""
fi

# Only run apt-get if any of the required tools are missing
NEED_APT=0
for cmd in git curl wget pip3; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        NEED_APT=1
        break
    fi
done

if [ $NEED_APT -eq 1 ] && command -v apt-get >/dev/null 2>&1; then
    echo "Installing missing system packages..."
    $SUDO apt-get update -qq
    $SUDO apt-get install -y -qq git curl wget python3-pip
else
    echo "System packages already present (skipping apt-get)"
fi

# 6. Python dependencies. The PyTorch base image already has torch + CUDA
#    so this only needs to install the lighter deps (transformers, peft,
#    boto3, wandb, etc.). Should take ~30s on the second run.
echo "Installing Python dependencies (this may take a few minutes the first time)..."
pip install --quiet --upgrade pip
pip install --quiet -r training/requirements.txt
echo "Python deps installed"

# 7. Verify CUDA / GPU
echo ""
echo "Checking GPU availability..."
python3 -c "
import torch
print(f'  PyTorch version:  {torch.__version__}')
print(f'  CUDA available:   {torch.cuda.is_available()}')
if torch.cuda.is_available():
    print(f'  CUDA version:     {torch.version.cuda}')
    print(f'  Device count:     {torch.cuda.device_count()}')
    for i in range(torch.cuda.device_count()):
        props = torch.cuda.get_device_properties(i)
        print(f'  Device {i}:         {props.name} ({props.total_memory // (1024**3)} GB)')
else:
    print('  WARNING: No CUDA device detected. Training will be unusably slow.')
"

# 8. Pull dataset from S3 — but skip if files already exist on the volume
#    (e.g., a previous pod already downloaded them).
echo ""
mkdir -p datasets/security/examples
TRAIN_PATH="datasets/security/examples/train.jsonl"
EVAL_PATH="datasets/security/examples/eval.jsonl"

if [ -s "$TRAIN_PATH" ] && [ -s "$EVAL_PATH" ]; then
    echo "Dataset already present on volume — skipping S3 download"
else
    echo "Pulling training dataset from S3..."
    python3 -c "
import sys
sys.path.insert(0, 'training')
from s3_utils import download_file
download_file('datasets/security/train.jsonl', 'datasets/security/examples/train.jsonl')
download_file('datasets/security/eval.jsonl',  'datasets/security/examples/eval.jsonl')
"
    echo "Dataset downloaded"
fi

# 9. Verify dataset
python3 -c "
from pathlib import Path
for name in ['train', 'eval']:
    path = Path(f'datasets/security/examples/{name}.jsonl')
    n = sum(1 for _ in path.open('r', encoding='utf-8'))
    size_mb = path.stat().st_size / (1024 * 1024)
    print(f'  {name}.jsonl: {n} lines, {size_mb:.1f} MB')
"

# 10. Verify wandb login (will use WANDB_API_KEY from .env)
echo ""
echo "Verifying Weights & Biases login..."
python3 -c "
import os, sys
sys.path.insert(0, 'training')
from s3_utils import load_env
load_env()
if os.environ.get('WANDB_DISABLED', '').lower() == 'true':
    print('  W&B disabled via WANDB_DISABLED')
elif os.environ.get('WANDB_API_KEY'):
    import wandb
    wandb.login(key=os.environ['WANDB_API_KEY'], verify=True)
    print('  W&B login OK')
else:
    print('  WARNING: WANDB_API_KEY not set; training will run without experiment tracking')
"

echo ""
echo "============================================"
echo "  Pod is ready. To start training:"
echo "    python training/train_security.py"
echo ""
echo "  To resume from the latest S3 checkpoint:"
echo "    python training/train_security.py --resume"
echo "============================================"
