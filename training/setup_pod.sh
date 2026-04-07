#!/usr/bin/env bash
# Lupus RunPod bootstrap script
#
# Run this on a fresh RunPod GPU instance after cloning the repo and
# placing your .env file at the repo root. It installs system and
# Python dependencies, pulls the dataset from S3, and verifies that
# the GPU is accessible.
#
# Usage on the pod:
#     git clone https://github.com/inviti8/lupus.git
#     cd lupus
#     # Copy your .env file into place (via runpodctl, scp, or paste)
#     bash training/setup_pod.sh
#
# Idempotent — safe to re-run.

set -euo pipefail

echo "============================================"
echo "  Lupus RunPod bootstrap"
echo "============================================"

# 1. Locate the repo root (the parent of training/)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
echo "Repo root: $REPO_ROOT"

# 2. Verify .env exists
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

# 3. System dependencies (most RunPod base images already have these,
#    but install if missing — apt-get is idempotent)
if command -v apt-get >/dev/null 2>&1; then
    echo "Installing system packages (apt-get)..."
    sudo apt-get update -qq
    sudo apt-get install -y -qq git curl wget python3-pip
fi

# 4. Python dependencies
echo "Installing Python dependencies (this may take a few minutes)..."
pip install --quiet --upgrade pip
pip install --quiet -r training/requirements.txt
echo "Python deps installed"

# 5. Verify CUDA / GPU
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

# 6. Pull dataset from S3
echo ""
echo "Pulling training dataset from S3..."
mkdir -p datasets/security/examples
python3 -c "
import sys
sys.path.insert(0, 'training')
from s3_utils import download_file
download_file('datasets/security/train.jsonl', 'datasets/security/examples/train.jsonl')
download_file('datasets/security/eval.jsonl',  'datasets/security/examples/eval.jsonl')
"
echo "Dataset downloaded"

# 7. Verify dataset
python3 -c "
from pathlib import Path
import json
for name in ['train', 'eval']:
    path = Path(f'datasets/security/examples/{name}.jsonl')
    n = sum(1 for _ in path.open('r', encoding='utf-8'))
    size_mb = path.stat().st_size / (1024 * 1024)
    print(f'  {name}.jsonl: {n} lines, {size_mb:.1f} MB')
"

# 8. Verify wandb login (will use WANDB_API_KEY from .env)
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
