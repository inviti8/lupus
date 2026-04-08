"""Pull a trained model from RunPod S3 to the local workstation.

Run this from your local workstation after training completes on the
RunPod side. Downloads the final model into ./dist/lupus-security/ by
default, ready for testing or for export to GGUF.

Usage:
    python training/pull_model.py [--model security] [--out dist/lupus-security]
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "training"))

from s3_utils import download_directory, list_objects  # noqa: E402

LOG = logging.getLogger("pull_model")

# Map friendly names to S3 prefixes
MODEL_PREFIXES = {
    "security": "models/lupus-security/final",
    "search": "models/lupus-search/final",
    "content": "models/lupus-content/final",
    "tinyagent": "models/lupus-tinyagent/final",
}

# Default local output directory per model. Falls back to dist/lupus-{model}
# if a model isn't listed here. tinyagent uses a -search suffix because it's
# the search-adapter LoRA; a future content adapter will be a separate dir.
MODEL_DEFAULT_OUT = {
    "tinyagent": "dist/lupus-tinyagent-search",
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--model",
        choices=list(MODEL_PREFIXES.keys()),
        default="security",
        help="Which model to pull (default: security)",
    )
    parser.add_argument(
        "--prefix",
        help="Override the S3 prefix to download (advanced)",
    )
    parser.add_argument(
        "--out",
        type=Path,
        help="Local output directory (default: dist/lupus-{model})",
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    s3_prefix = args.prefix or MODEL_PREFIXES[args.model]
    if args.out:
        out_dir = args.out
    elif args.model in MODEL_DEFAULT_OUT:
        out_dir = REPO_ROOT / MODEL_DEFAULT_OUT[args.model]
    else:
        out_dir = REPO_ROOT / "dist" / f"lupus-{args.model}"

    # Verify there's something to download
    objs = list_objects(prefix=s3_prefix.rstrip("/") + "/")
    if not objs:
        LOG.error("No objects found at s3://.../%s", s3_prefix)
        LOG.error("Has training completed and pushed the final model?")
        return 1

    LOG.info("Found %d files to download (%.1f MB total)",
             len(objs), sum(o["Size"] for o in objs) / (1024 * 1024))
    download_directory(s3_prefix, out_dir)
    LOG.info("Done. Model is at: %s", out_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
