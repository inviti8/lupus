"""Deploy a Lupus training pod on RunPod with auto-retry.

Uses the REST API at POST /pods with smart auto-failover:
  - Specifies acceptable GPU types as a list
  - Specifies acceptable data centers as a list
  - Sets dataCenterPriority and gpuTypePriority to "availability"
  - RunPod picks the first available match

If no capacity is available right now, the script polls every N seconds
and retries until it succeeds. On success, it prints the pod ID, the
SSH/Web Terminal URLs, and the exact next-step commands to bootstrap
training.

Defaults are tuned for the Lupus security model training:
  - Volume:  the existing 7oqdtnkk5f volume (US-IL-1)
  - GPUs:    NVIDIA GeForce RTX 4090
  - Image:   runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04
  - Disk:    20 GB container disk
  - Spot:    interruptible, max bid $0.30/hr
  - Name:    lupus-training-{timestamp}

Usage:
    python tools/runpod_deploy.py                        # use defaults
    python tools/runpod_deploy.py --on-demand            # use on-demand instead of spot
    python tools/runpod_deploy.py --max-bid 0.40
    python tools/runpod_deploy.py --gpu "NVIDIA RTX A6000" --gpu "NVIDIA GeForce RTX 4090"
    python tools/runpod_deploy.py --no-retry             # one attempt, no polling
    python tools/runpod_deploy.py --dry-run              # print the request body, don't deploy

WARNING: this script CREATES a pod which immediately starts billing.
Use --dry-run first to verify the request body before running for real.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Optional

import requests
from dotenv import load_dotenv

if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[1]
load_dotenv(REPO_ROOT / ".env")

API_KEY = os.environ.get("RUNPOD_API_KEY")
if not API_KEY:
    print("ERROR: RUNPOD_API_KEY not set in .env", file=sys.stderr)
    sys.exit(1)

REST_BASE = "https://rest.runpod.io/v1"
REST_HEADERS = {"Authorization": f"Bearer {API_KEY}", "Content-Type": "application/json"}


# ---------------------------------------------------------------------------
# Defaults — tuned for Lupus training
# ---------------------------------------------------------------------------


DEFAULTS = {
    "name_prefix": "lupus-training",
    "gpu_type_ids": ["NVIDIA GeForce RTX 4090"],
    "data_center_ids": ["US-IL-1"],
    "network_volume_id": "7oqdtnkk5f",
    "image_name": "runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04",
    "container_disk_in_gb": 20,
    "volume_mount_path": "/workspace",
    "gpu_count": 1,
    "min_vcpu_per_gpu": 4,
    "min_ram_per_gpu": 16,
    "ports": ["22/tcp", "8888/http"],
    "interruptible_max_bid": 0.30,
    "poll_interval_sec": 30,
    "max_attempts": 60,  # 30 min of polling at 30s interval
}


# ---------------------------------------------------------------------------
# REST helpers
# ---------------------------------------------------------------------------


def rest(method: str, path: str, **kwargs):
    url = f"{REST_BASE}{path}"
    resp = requests.request(method, url, headers=REST_HEADERS, timeout=60, **kwargs)
    if resp.status_code == 204:
        return None, resp.status_code
    try:
        return resp.json(), resp.status_code
    except json.JSONDecodeError:
        return resp.text, resp.status_code


# ---------------------------------------------------------------------------
# Pod creation
# ---------------------------------------------------------------------------


def build_pod_request(args) -> dict:
    """Build the JSON request body for POST /pods."""
    name = args.name or f"{DEFAULTS['name_prefix']}-{datetime.now().strftime('%Y%m%d-%H%M%S')}"

    body = {
        "name": name,
        "computeType": "GPU",
        "gpuTypeIds": args.gpu or DEFAULTS["gpu_type_ids"],
        "gpuCount": args.gpu_count,
        "gpuTypePriority": "availability",
        "dataCenterIds": args.region or DEFAULTS["data_center_ids"],
        "dataCenterPriority": "availability",
        "interruptible": not args.on_demand,
        "imageName": args.image,
        "containerDiskInGb": args.container_disk,
        "minVCPUPerGPU": DEFAULTS["min_vcpu_per_gpu"],
        "minRAMPerGPU": DEFAULTS["min_ram_per_gpu"],
        "ports": DEFAULTS["ports"],
    }

    # Network volume attachment (also sets the mount path)
    if args.volume:
        body["networkVolumeId"] = args.volume
        body["volumeMountPath"] = DEFAULTS["volume_mount_path"]

    # Cloud type — SECURE for the volume's region (US-IL-1 only has SECURE)
    body["cloudType"] = args.cloud_type

    return body


def attempt_deploy(body: dict) -> tuple[Optional[dict], Optional[str]]:
    """Try to create the pod once. Returns (pod, error_message)."""
    result, status = rest("POST", "/pods", json=body)

    if status == 200 or status == 201:
        return result, None

    # Extract a useful error message
    if isinstance(result, dict):
        msg = result.get("error") or result.get("message") or json.dumps(result)
    else:
        msg = str(result)

    return None, f"HTTP {status}: {msg}"


def deploy_with_retry(body: dict, max_attempts: int, poll_interval: int) -> Optional[dict]:
    """Try to deploy, polling on capacity errors."""
    capacity_error_keywords = (
        "no longer any instances available",
        "no instances available",
        "capacity",
        "out of stock",
        "unavailable",
        "not available",
        "no machines",
    )

    for attempt in range(1, max_attempts + 1):
        ts = datetime.now().strftime("%H:%M:%S")
        print(f"[{ts}] Attempt {attempt}/{max_attempts}: deploying...", flush=True)
        pod, error = attempt_deploy(body)

        if pod is not None:
            print(f"[{ts}] ✓ DEPLOYED: pod {pod.get('id')}", flush=True)
            return pod

        # Determine whether to retry
        if any(kw in (error or "").lower() for kw in capacity_error_keywords):
            print(f"[{ts}]   no capacity right now: {error}", flush=True)
            if attempt < max_attempts:
                print(f"[{ts}]   waiting {poll_interval}s before retry...", flush=True)
                time.sleep(poll_interval)
            continue

        # Non-capacity error — fail fast
        print(f"[{ts}] ✗ DEPLOY ERROR (not retryable): {error}", flush=True)
        return None

    print(f"[{ts}] ✗ Gave up after {max_attempts} attempts", flush=True)
    return None


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def print_pod_summary(pod: dict) -> None:
    print()
    print("=" * 70)
    print("  Pod is up")
    print("=" * 70)
    print(f"  ID:           {pod.get('id')}")
    print(f"  Name:         {pod.get('name')}")
    print(f"  Status:       {pod.get('desiredStatus')}")
    if "machine" in pod and isinstance(pod["machine"], dict):
        m = pod["machine"]
        print(f"  GPU:          {m.get('gpuTypeId')}")
        print(f"  Datacenter:   {m.get('dataCenterId')}")
    if "costPerHr" in pod:
        print(f"  Cost/hr:      ${pod['costPerHr']:.3f}")
    print()
    print("  Web console:  https://www.runpod.io/console/pods")
    print()
    print("  Connect via web terminal:")
    print(f"    1. Open https://www.runpod.io/console/pods/{pod.get('id')}")
    print( "    2. Click 'Connect' → 'Start Web Terminal'")
    print()
    print("  Once in the terminal:")
    print( "    apt-get update && apt-get install -y nano tmux")
    print( "    cd /workspace")
    print( "    rm -rf lupus")
    print( "    git clone https://github.com/inviti8/lupus.git")
    print( "    cd lupus")
    print( "    # paste your .env here using cat > .env <<EOF ... EOF")
    print( "    tmux new -s lupus")
    print( "    bash training/setup_pod.sh")
    print( "    python training/train_security.py")
    print( "    # Detach with Ctrl-B then D")
    print()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", help="Pod name (default: lupus-training-<timestamp>)")
    parser.add_argument("--gpu", action="append",
                        help="GPU type ID (repeatable). Default: NVIDIA GeForce RTX 4090")
    parser.add_argument("--gpu-count", type=int, default=DEFAULTS["gpu_count"])
    parser.add_argument("--region", action="append",
                        help="Data center ID (repeatable). Default: US-IL-1")
    parser.add_argument("--volume", default=DEFAULTS["network_volume_id"],
                        help=f"Network volume ID (default: {DEFAULTS['network_volume_id']})")
    parser.add_argument("--no-volume", action="store_true",
                        help="Don't attach a network volume (ephemeral pod)")
    parser.add_argument("--image", default=DEFAULTS["image_name"],
                        help="Docker image to run")
    parser.add_argument("--container-disk", type=int, default=DEFAULTS["container_disk_in_gb"],
                        help="Container disk size in GB")
    parser.add_argument("--on-demand", action="store_true",
                        help="Use on-demand pricing instead of interruptable spot")
    parser.add_argument("--max-bid", type=float, default=DEFAULTS["interruptible_max_bid"],
                        help="Max spot bid per GPU per hour")
    parser.add_argument("--cloud-type", choices=["SECURE", "COMMUNITY", "ALL"], default="SECURE",
                        help="Cloud type (SECURE for the network volume to attach)")
    parser.add_argument("--no-retry", action="store_true",
                        help="One deploy attempt, don't poll on capacity")
    parser.add_argument("--max-attempts", type=int, default=DEFAULTS["max_attempts"],
                        help="Max deploy attempts when polling")
    parser.add_argument("--poll-interval", type=int, default=DEFAULTS["poll_interval_sec"],
                        help="Seconds between deploy retries")
    parser.add_argument("--dry-run", action="store_true",
                        help="Print the request body, don't actually deploy")
    args = parser.parse_args()

    if args.no_volume:
        args.volume = None

    body = build_pod_request(args)

    print("Deploy request:")
    print(json.dumps(body, indent=2))
    print()

    if args.dry_run:
        print("DRY RUN — not deploying. Re-run without --dry-run to deploy for real.")
        return 0

    print("Submitting deploy to RunPod...")
    print()

    if args.no_retry:
        pod, error = attempt_deploy(body)
        if pod is None:
            print(f"FAILED: {error}", file=sys.stderr)
            return 1
        print_pod_summary(pod)
        return 0

    pod = deploy_with_retry(body, args.max_attempts, args.poll_interval)
    if pod is None:
        return 1
    print_pod_summary(pod)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
