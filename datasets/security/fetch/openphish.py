"""OpenPhish fetcher — downloads the public phishing URL feed.

OpenPhish provides a free, no-registration phishing URL feed at
https://openphish.com/feed.txt. The free feed is much smaller than
PhishTank (typically ~500 currently active phishing URLs at any time),
but it's available without registration and is a useful complement to
or fallback for PhishTank.

The feed is plain text — one URL per line, no metadata. The fetcher
saves it as a CSV with a single 'url' column for consistency with the
other source CSVs.

Usage:
    python openphish.py [--output PATH] [--force]

Note: For larger datasets, OpenPhish offers paid premium feeds with
historical data and metadata. This fetcher only uses the free feed.
"""

from __future__ import annotations

import argparse
import csv
import logging
from pathlib import Path

import requests

LOG = logging.getLogger("openphish")

FEED_URL = "https://openphish.com/feed.txt"

USER_AGENT = "lupus-security-research/0.1 (+https://github.com/inviti8/lupus)"


def download(url: str, output_path: Path, force: bool = False) -> int:
    """Download the OpenPhish feed and save as CSV. Returns the number of URLs."""
    if output_path.exists() and not force:
        # Count existing rows
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
    resp = requests.get(url, headers=headers, timeout=30)
    resp.raise_for_status()

    # Plain text, one URL per line
    urls = [line.strip() for line in resp.text.splitlines() if line.strip()]
    LOG.info("Got %d phishing URLs from OpenPhish", len(urls))

    # Write as CSV with a single 'url' column for consistency with the
    # other source CSVs (and so build_dataset.py can read them uniformly)
    with output_path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["url"])
        for u in urls:
            writer.writerow([u])

    LOG.info("Wrote %d URLs to %s", len(urls), output_path)
    return len(urls)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).parent.parent / "raw" / "openphish.csv",
        help="Output path (default: ../raw/openphish.csv)",
    )
    parser.add_argument("--force", action="store_true", help="Overwrite existing file")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    try:
        download(FEED_URL, args.output, force=args.force)
    except requests.HTTPError as e:
        LOG.error("HTTP error: %s", e)
        return 1
    except requests.RequestException as e:
        LOG.error("Network error: %s", e)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
