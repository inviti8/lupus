"""Phishing.Database fetcher — downloads from the Phishing-Database/Phishing.Database GitHub repo.

A community-maintained phishing URL aggregator that pulls from multiple
upstream sources (OpenPhish, URLhaus, AbuseIPDB, Cisco Umbrella, and
others) into a single regularly-updated list. Free, no registration,
much easier to use than PhishTank since 2025. Validates URLs with the
PyFunceble testing tool and is an official data vendor for VirusTotal.

Repo: https://github.com/Phishing-Database/Phishing.Database
  (was originally mitchellkrogza/Phishing.Database, moved to the
   Phishing-Database org in 2024)
License: MIT
Update frequency: Multiple times daily (auto-updated by GitHub Actions)
Maintainers: Mitchell Krog (@mitchellkrogza), Nissar Chababy (@funilrys)

Available lists:
    active   — phishing-links-ACTIVE.txt           (currently-online URLs, ~30K)
    new      — phishing-links-NEW-today.txt        (added today, ~hundreds)
    all      — ALL-phishing-links.txt              (full archive, ~430K)
    domains  — ALL-phishing-domains.txt            (domains only, ~270K)

Usage:
    python phishing_database.py                           # default: active list
    python phishing_database.py --list all                # full archive
    python phishing_database.py --list domains            # domains only (no full URLs)
    python phishing_database.py --output ../raw/foo.csv
"""

from __future__ import annotations

import argparse
import csv
import logging
from pathlib import Path

import requests

LOG = logging.getLogger("phishing_database")

USER_AGENT = "lupus-security-research/0.1 (+https://github.com/inviti8/lupus)"

REPO = "Phishing-Database/Phishing.Database"
BRANCH = "master"
RAW_BASE = f"https://raw.githubusercontent.com/{REPO}/{BRANCH}"

LIST_URLS = {
    "active": f"{RAW_BASE}/phishing-links-ACTIVE.txt",
    "new": f"{RAW_BASE}/phishing-links-NEW-today.txt",
    "all": f"{RAW_BASE}/ALL-phishing-links.txt",
    "domains": f"{RAW_BASE}/ALL-phishing-domains.txt",
    "domains-active": f"{RAW_BASE}/phishing-domains-ACTIVE.txt",
}


def download(url: str, output_path: Path, force: bool = False) -> int:
    """Download a phishing list and save as CSV. Returns the number of URLs."""
    if output_path.exists() and not force:
        with output_path.open("r", encoding="utf-8") as f:
            existing = sum(1 for _ in f) - 1  # minus header
        LOG.info(
            "Output already exists at %s (%d URLs); use --force to overwrite",
            output_path, existing,
        )
        return existing

    output_path.parent.mkdir(parents=True, exist_ok=True)
    LOG.info("Downloading %s", url)

    headers = {"User-Agent": USER_AGENT}
    resp = requests.get(url, headers=headers, timeout=120)
    resp.raise_for_status()

    # Plain text, one URL/domain per line. Strip empty lines and comments.
    urls = [
        line.strip()
        for line in resp.text.splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]
    LOG.info("Got %d phishing entries", len(urls))

    # Write as CSV with a single 'url' column for consistency with the
    # other source CSVs (and so build_dataset.py can read them uniformly)
    with output_path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["url"])
        for u in urls:
            writer.writerow([u])

    LOG.info("Wrote %d entries to %s", len(urls), output_path)
    return len(urls)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--list",
        choices=list(LIST_URLS.keys()),
        default="active",
        help="Which list to download (default: active)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).parent.parent / "raw" / "phishing_database.csv",
        help="Output path (default: ../raw/phishing_database.csv)",
    )
    parser.add_argument("--force", action="store_true", help="Overwrite existing file")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    url = LIST_URLS[args.list]

    try:
        download(url, args.output, force=args.force)
    except requests.HTTPError as e:
        LOG.error("HTTP error: %s", e)
        return 1
    except requests.RequestException as e:
        LOG.error("Network error: %s", e)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
