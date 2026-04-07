# Lupus Training Infrastructure

Code and documentation for training the Lupus models on RunPod.

**Start here:** [`RUNBOOK.md`](RUNBOOK.md) — the detailed step-by-step runbook for training the security model end-to-end. Designed to be self-contained so you can follow it on a RunPod instance without flipping back to other docs.

## Files

| File | Purpose | Where it runs |
|---|---|---|
| `RUNBOOK.md` | The complete training workflow as a runbook | n/a (read it) |
| `requirements.txt` | Python dependencies for the GPU pod | RunPod |
| `s3_utils.py` | RunPod S3-compatible storage helpers | both |
| `push_dataset.py` | Upload `datasets/security/examples/*.jsonl` to S3 | local workstation |
| `pull_model.py` | Download the trained model from S3 | local workstation |
| `setup_pod.sh` | Bootstrap a fresh RunPod instance | RunPod |
| `train_security.py` | Main training script (Qwen2.5-Coder-0.5B classifier) | RunPod |
| `output/` | Local checkpoint directory (gitignored) | RunPod |

## Quick reference

```bash
# Local — push dataset to S3 (one-time, ~25MB)
python training/push_dataset.py

# On the pod — bootstrap and start training
bash training/setup_pod.sh
python training/train_security.py

# Resume after spot interruption
python training/train_security.py --resume

# Local — pull the trained model
python training/pull_model.py
```

For everything you need to know — provisioning the RunPod instance, copying credentials, monitoring training, recovering from interruptions, and post-training validation — see [`RUNBOOK.md`](RUNBOOK.md).
