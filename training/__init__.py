"""Lupus training infrastructure.

Modules:
    s3_utils       — push/pull data and checkpoints to RunPod S3
    push_dataset   — local script: upload datasets/security to S3
    pull_model     — local script: download trained model from S3
    train_security — main training script for the security classifier

See RUNBOOK.md for the full step-by-step training workflow.
"""
