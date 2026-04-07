"""S3 utility functions for the Lupus training pipeline.

Wraps boto3 with the RunPod S3-compatible endpoint and provides
high-level helpers for the operations the training pipeline needs:
upload/download files and directories, find latest checkpoint,
and basic existence checks.

All credentials come from the .env file (loaded via python-dotenv)
or from the process environment if .env is not present.

Required env vars:
    S3_ENDPOINT_URL       e.g. https://s3api-us-il-1.runpod.io
    S3_BUCKET             e.g. 7oqdtnkk5f
    AWS_ACCESS_KEY_ID
    AWS_SECRET_ACCESS_KEY
    AWS_DEFAULT_REGION    e.g. us-il-1
"""

from __future__ import annotations

import logging
import os
import re
from pathlib import Path
from typing import Iterable, Optional

import boto3
from botocore.client import Config
from botocore.exceptions import ClientError, EndpointConnectionError

LOG = logging.getLogger("s3_utils")


# ---------------------------------------------------------------------------
# Client setup
# ---------------------------------------------------------------------------


def load_env() -> None:
    """Load .env from the repo root if not already loaded."""
    try:
        from dotenv import load_dotenv
    except ImportError:
        return  # silently fall back to process environment
    # Repo root is two levels up from this file
    env_path = Path(__file__).resolve().parents[1] / ".env"
    if env_path.exists():
        load_dotenv(env_path)


def get_s3_client():
    """Build a boto3 S3 client configured for the RunPod endpoint."""
    load_env()
    endpoint = os.environ.get("S3_ENDPOINT_URL")
    region = os.environ.get("AWS_DEFAULT_REGION", "us-east-1")
    if not endpoint:
        raise RuntimeError(
            "S3_ENDPOINT_URL not set. Did you create .env from .env.example?"
        )
    return boto3.client(
        "s3",
        endpoint_url=endpoint,
        region_name=region,
        config=Config(retries={"max_attempts": 5, "mode": "adaptive"}),
    )


def get_bucket() -> str:
    """Return the configured S3 bucket name."""
    load_env()
    bucket = os.environ.get("S3_BUCKET")
    if not bucket:
        raise RuntimeError("S3_BUCKET not set in .env")
    return bucket


# ---------------------------------------------------------------------------
# File operations
# ---------------------------------------------------------------------------


def upload_file(local_path: Path | str, s3_key: str) -> int:
    """Upload a single file to S3. Returns the file size in bytes."""
    local_path = Path(local_path)
    if not local_path.is_file():
        raise FileNotFoundError(local_path)
    s3 = get_s3_client()
    bucket = get_bucket()
    size = local_path.stat().st_size
    LOG.info("Uploading %s (%.1f KB) → s3://%s/%s", local_path, size / 1024, bucket, s3_key)
    s3.upload_file(str(local_path), bucket, s3_key)
    return size


def download_file(s3_key: str, local_path: Path | str) -> int:
    """Download a single S3 object to a local path. Returns size in bytes."""
    local_path = Path(local_path)
    local_path.parent.mkdir(parents=True, exist_ok=True)
    s3 = get_s3_client()
    bucket = get_bucket()
    LOG.info("Downloading s3://%s/%s → %s", bucket, s3_key, local_path)
    s3.download_file(bucket, s3_key, str(local_path))
    return local_path.stat().st_size


def upload_directory(local_dir: Path | str, s3_prefix: str) -> int:
    """Recursively upload a directory tree to S3 under the given prefix.

    Returns the total bytes uploaded. The S3 keys mirror the relative path
    structure beneath local_dir, joined with s3_prefix.
    """
    local_dir = Path(local_dir)
    if not local_dir.is_dir():
        raise NotADirectoryError(local_dir)
    s3 = get_s3_client()
    bucket = get_bucket()

    # Normalize prefix
    s3_prefix = s3_prefix.rstrip("/")
    total = 0
    count = 0
    for path in sorted(local_dir.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(local_dir).as_posix()
        key = f"{s3_prefix}/{rel}"
        s3.upload_file(str(path), bucket, key)
        total += path.stat().st_size
        count += 1
    LOG.info(
        "Uploaded %d files (%.1f MB) from %s → s3://%s/%s/",
        count, total / (1024 * 1024), local_dir, bucket, s3_prefix,
    )
    return total


def download_directory(s3_prefix: str, local_dir: Path | str) -> int:
    """Recursively download an S3 prefix to a local directory.

    Returns the total bytes downloaded.
    """
    local_dir = Path(local_dir)
    local_dir.mkdir(parents=True, exist_ok=True)
    s3 = get_s3_client()
    bucket = get_bucket()

    s3_prefix = s3_prefix.rstrip("/") + "/"
    paginator = s3.get_paginator("list_objects_v2")
    total = 0
    count = 0
    for page in paginator.paginate(Bucket=bucket, Prefix=s3_prefix):
        for obj in page.get("Contents", []):
            key = obj["Key"]
            rel = key[len(s3_prefix):]
            if not rel:
                continue
            dest = local_dir / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            s3.download_file(bucket, key, str(dest))
            total += obj["Size"]
            count += 1
    LOG.info(
        "Downloaded %d files (%.1f MB) from s3://%s/%s → %s",
        count, total / (1024 * 1024), bucket, s3_prefix, local_dir,
    )
    return total


# ---------------------------------------------------------------------------
# Listing / discovery
# ---------------------------------------------------------------------------


def list_objects(prefix: str = "") -> list[dict]:
    """List all objects under a prefix. Returns full S3 metadata records."""
    s3 = get_s3_client()
    bucket = get_bucket()
    paginator = s3.get_paginator("list_objects_v2")
    results: list[dict] = []
    for page in paginator.paginate(Bucket=bucket, Prefix=prefix):
        results.extend(page.get("Contents", []))
    return results


def find_latest_checkpoint(s3_prefix: str) -> Optional[str]:
    """Find the highest-numbered HuggingFace checkpoint under an S3 prefix.

    HF Trainer saves checkpoints as `checkpoint-{step}/` directories. This
    function returns the prefix of the latest one (highest step number),
    or None if no checkpoints exist under the given prefix.
    """
    s3_prefix = s3_prefix.rstrip("/")
    objs = list_objects(prefix=f"{s3_prefix}/checkpoint-")

    # Extract distinct checkpoint directory names
    pattern = re.compile(rf"^{re.escape(s3_prefix)}/checkpoint-(\d+)/")
    steps: dict[int, str] = {}
    for obj in objs:
        m = pattern.match(obj["Key"])
        if m:
            step = int(m.group(1))
            steps[step] = f"{s3_prefix}/checkpoint-{step}"

    if not steps:
        return None
    latest = max(steps.keys())
    return steps[latest]


def object_exists(s3_key: str) -> bool:
    """Check whether a single S3 object exists."""
    s3 = get_s3_client()
    bucket = get_bucket()
    try:
        s3.head_object(Bucket=bucket, Key=s3_key)
        return True
    except ClientError as e:
        if e.response.get("Error", {}).get("Code") in ("404", "NoSuchKey", "NotFound"):
            return False
        raise


# ---------------------------------------------------------------------------
# CLI for sanity checks
# ---------------------------------------------------------------------------


def main() -> int:
    """`python s3_utils.py` — quick connection check + bucket listing."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    try:
        objs = list_objects()
        bucket = get_bucket()
        print(f"Connected to s3://{bucket}/")
        print(f"  {len(objs)} object(s) total")
        for obj in objs[:20]:
            print(f"  - {obj['Key']} ({obj['Size'] / 1024:.1f} KB)")
        if len(objs) > 20:
            print(f"  ... and {len(objs) - 20} more")
        return 0
    except (EndpointConnectionError, ClientError, RuntimeError) as e:
        print(f"ERROR: {e}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
