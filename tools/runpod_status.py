"""Read-only RunPod account status.

Prints:
  - Account info (email, current spend rate)
  - Active pods (status, GPU, region)
  - Network volumes (id, name, size, region)
  - GPU stock for our preferred types in our preferred region

Safe to run anytime — makes no changes.

Usage:
    python tools/runpod_status.py
    python tools/runpod_status.py --region US-IL-1
    python tools/runpod_status.py --gpu "NVIDIA GeForce RTX 4090"
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path
from typing import Optional

import requests
from dotenv import load_dotenv

# UTF-8 stdout on Windows so the output renders cleanly
if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[1]
load_dotenv(REPO_ROOT / ".env")

API_KEY = os.environ.get("RUNPOD_API_KEY")
if not API_KEY:
    print("ERROR: RUNPOD_API_KEY not set in .env", file=sys.stderr)
    sys.exit(1)

GRAPHQL_URL = f"https://api.runpod.io/graphql?api_key={API_KEY}"
REST_BASE = "https://rest.runpod.io/v1"
REST_HEADERS = {"Authorization": f"Bearer {API_KEY}", "Content-Type": "application/json"}

DEFAULT_REGION = "US-IL-1"
DEFAULT_GPUS = [
    "NVIDIA GeForce RTX 4090",
    "NVIDIA RTX A6000",
    "NVIDIA RTX A5000",
    "NVIDIA A40",
    "NVIDIA L40S",
]


def gql(query: str, variables: dict | None = None) -> dict:
    payload = {"query": query}
    if variables:
        payload["variables"] = variables
    resp = requests.post(GRAPHQL_URL, json=payload, timeout=30)
    resp.raise_for_status()
    body = resp.json()
    if "errors" in body:
        raise RuntimeError(f"GraphQL: {body['errors']}")
    return body["data"]


def rest(method: str, path: str, **kwargs):
    url = f"{REST_BASE}{path}"
    resp = requests.request(method, url, headers=REST_HEADERS, timeout=30, **kwargs)
    if resp.status_code == 204:
        return None
    resp.raise_for_status()
    return resp.json()


def section(title: str) -> None:
    print()
    print("=" * 70)
    print(f"  {title}")
    print("=" * 70)


def print_account() -> None:
    section("Account")
    me = gql("""
        query Me {
            myself {
                id
                email
                currentSpendPerHr
                machineQuota
            }
        }
    """)["myself"]
    print(f"  Email:        {me.get('email')}")
    print(f"  User ID:      {me.get('id')}")
    print(f"  Spend rate:   ${me.get('currentSpendPerHr', 0):.4f}/hr")
    print(f"  Machine quota: {me.get('machineQuota')}")


def print_pods() -> None:
    section("Active pods")
    pods = rest("GET", "/pods")
    if not isinstance(pods, list) or len(pods) == 0:
        print("  No active pods.")
        return
    for p in pods:
        print(f"  - {p.get('id')}")
        print(f"      name:        {p.get('name')}")
        print(f"      status:      {p.get('desiredStatus')}")
        print(f"      gpu:         {p.get('machine', {}).get('gpuTypeId') if p.get('machine') else '?'}")
        print(f"      cost/hr:     ${p.get('costPerHr', 0):.3f}")


def print_volumes() -> None:
    section("Network volumes")
    vols = rest("GET", "/networkvolumes")
    if not isinstance(vols, list) or len(vols) == 0:
        print("  No network volumes.")
        return
    for v in vols:
        print(f"  - {v.get('id')}")
        print(f"      name:        {v.get('name')}")
        print(f"      size:        {v.get('size')} GB")
        print(f"      region:      {v.get('dataCenterId')}")


def print_gpu_availability(region: str, gpu_ids: list[str]) -> None:
    section(f"GPU availability in {region}")
    print(f"  {'GPU':<42} {'Stock':<12} {'SpotBid':>10} {'OnDemand':>10}")
    print(f"  {'-'*42} {'-'*12} {'-'*10} {'-'*10}")

    found_any = False
    for gpu_id in gpu_ids:
        try:
            data = gql("""
                query GpuRegion($id: String, $dc: String) {
                    gpuTypes(input: { id: $id }) {
                        id
                        memoryInGb
                        lowestPrice(input: { gpuCount: 1, dataCenterId: $dc }) {
                            minimumBidPrice
                            uninterruptablePrice
                            stockStatus
                        }
                    }
                }
            """, variables={"id": gpu_id, "dc": region})
            results = data.get("gpuTypes") or []
            if not results:
                continue
            for g in results:
                lp = g.get("lowestPrice") or {}
                stock = lp.get("stockStatus")
                bid = lp.get("minimumBidPrice")
                ondemand = lp.get("uninterruptablePrice")
                stock_s = stock if stock else "—"
                bid_s = f"${bid:.3f}/hr" if bid is not None else "—"
                ondemand_s = f"${ondemand:.3f}/hr" if ondemand is not None else "—"
                if stock and stock != "Unavailable":
                    found_any = True
                print(f"  {g['id'][:42]:<42} {stock_s:<12} {bid_s:>10} {ondemand_s:>10}")
        except Exception as e:
            print(f"  {gpu_id}: ERROR {e}")

    if not found_any:
        print()
        print(f"  ⚠ No GPUs currently available in {region}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--region", default=DEFAULT_REGION,
                        help=f"Data center region (default: {DEFAULT_REGION})")
    parser.add_argument("--gpu", action="append",
                        help="GPU type to check (repeatable). Defaults to common ones.")
    parser.add_argument("--quick", action="store_true",
                        help="Only show GPU availability (skip account/pods/volumes)")
    args = parser.parse_args()

    gpu_ids = args.gpu or DEFAULT_GPUS

    if not args.quick:
        print_account()
        print_pods()
        print_volumes()
    print_gpu_availability(args.region, gpu_ids)

    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
