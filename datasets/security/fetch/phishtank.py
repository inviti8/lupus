"""PhishTank fetcher — downloads verified phishing URLs.

PhishTank now requires a free registered API key for bulk CSV downloads.
The historical anonymous endpoint (http://data.phishtank.com/data/online-valid.csv)
redirects to a signed CDN URL that returns 404 without authentication.

To use this fetcher:
  1. Register for a free account at https://phishtank.org/
  2. Generate an application key in your account settings
  3. Run: python phishtank.py --api-key YOUR_KEY
     Or set the PHISHTANK_API_KEY environment variable.

If you don't want to register, use openphish.py instead — it provides
a smaller but free no-registration phishing feed.

Usage:
    python phishtank.py --api-key YOUR_KEY [--output PATH] [--force]
    PHISHTANK_API_KEY=YOUR_KEY python phishtank.py
"""

from __future__ import annotations

import argparse
import logging
import os
import sys
from pathlib import Path

import requests

LOG = logging.getLogger("phishtank")

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
        help="PhishTank API key (or set PHISHTANK_API_KEY env var). REQUIRED.",
    )
    parser.add_argument("--force", action="store_true", help="Overwrite existing file")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    api_key = args.api_key or os.environ.get("PHISHTANK_API_KEY")
    if not api_key:
        LOG.error(
            "PhishTank requires a free registered API key for bulk downloads. "
            "Register at https://phishtank.org/ and pass --api-key, or set "
            "the PHISHTANK_API_KEY environment variable. "
            "Alternatively, use openphish.py for a no-registration phishing feed."
        )
        return 1

    url = KEYED_URL.format(api_key=api_key)

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
