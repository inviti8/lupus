"""Push the security dataset to RunPod S3.

Run this once from your local workstation after building the dataset
with `python datasets/security/build_dataset.py`. Uploads:

    datasets/security/examples/train.jsonl  →  s3://{bucket}/datasets/security/train.jsonl
    datasets/security/examples/eval.jsonl   →  s3://{bucket}/datasets/security/eval.jsonl

The schema definition is also pushed for reference and reproducibility:
    datasets/security/schema.py             →  s3://{bucket}/datasets/security/schema.py

Usage:
    python training/push_dataset.py [--force]
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

# Allow running from anywhere — find the repo root and add it to the path
REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "training"))

from s3_utils import object_exists, upload_file  # noqa: E402

LOG = logging.getLogger("push_dataset")


FILES_TO_PUSH = [
    (REPO_ROOT / "datasets" / "security" / "examples" / "train.jsonl",
     "datasets/security/train.jsonl"),
    (REPO_ROOT / "datasets" / "security" / "examples" / "eval.jsonl",
     "datasets/security/eval.jsonl"),
    (REPO_ROOT / "datasets" / "security" / "schema.py",
     "datasets/security/schema.py"),
]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--force",
        action="store_true",
        help="Re-upload even if the S3 object already exists",
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    total_bytes = 0
    pushed = 0
    skipped = 0
    missing = 0

    for local_path, s3_key in FILES_TO_PUSH:
        if not local_path.exists():
            LOG.error("Missing local file: %s", local_path)
            LOG.error("  Run `python datasets/security/build_dataset.py` first")
            missing += 1
            continue

        if not args.force and object_exists(s3_key):
            LOG.info("Already exists in S3 (use --force to overwrite): %s", s3_key)
            skipped += 1
            continue

        size = upload_file(local_path, s3_key)
        total_bytes += size
        pushed += 1

    LOG.info("---")
    LOG.info("Pushed: %d  Skipped: %d  Missing: %d", pushed, skipped, missing)
    LOG.info("Total uploaded: %.2f MB", total_bytes / (1024 * 1024))

    if missing > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
