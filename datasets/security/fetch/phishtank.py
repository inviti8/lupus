"""PhishTank fetcher — downloads verified phishing URLs.

PhishTank provides a daily-updated CSV of verified phishing URLs at
http://data.phishtank.com/data/online-valid.csv (anonymous access).

With an API key, use the keyed endpoint for higher rate limits:
http://data.phishtank.com/data/{apikey}/online-valid.csv

Usage:
    python phishtank.py [--output PATH] [--api-key KEY] [--force]
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

import requests

LOG = logging.getLogger("phishtank")

ANON_URL = "http://data.phishtank.com/data/online-valid.csv"
KEYED_URL = "http://data.phishtank.com/data/{api_key}/online-valid.csv"

USER_AGENT = "lupus-security-research/0.1 (+https://github.com/inviti8/lupus)"


def download(url: str, output_path: Path, force: bool = False) -> int:
    """Download the PhishTank CSV. Returns the number of bytes written."""
    if output_path.exists() and not force:
        size = output_path.stat().st_size
        LOG.info("Output already exists at %s (%d bytes); use --force to overwrite", output_path, size)
        return size

    output_path.parent.mkdir(parents=True, exist_ok=True)
    LOG.info("Downloading %s", url)

    headers = {"User-Agent": USER_AGENT}
    with requests.get(url, headers=headers, stream=True, timeout=30) as resp:
        resp.raise_for_status()
        bytes_written = 0
        with output_path.open("wb") as f:
            for chunk in resp.iter_content(chunk_size=65536):
                f.write(chunk)
                bytes_written += len(chunk)

    LOG.info("Wrote %d bytes to %s", bytes_written, output_path)
    return bytes_written


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).parent.parent / "raw" / "phishtank.csv",
        help="Output path (default: ../raw/phishtank.csv)",
    )
    parser.add_argument(
        "--api-key",
        help="PhishTank API key (optional, gives higher rate limits)",
    )
    parser.add_argument("--force", action="store_true", help="Overwrite existing file")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    url = KEYED_URL.format(api_key=args.api_key) if args.api_key else ANON_URL

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
