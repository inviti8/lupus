"""URLhaus fetcher — downloads malware distribution URLs from abuse.ch.

URLhaus is a project of abuse.ch that tracks URLs distributing malware.
Free, no registration required, CC-0 licensed.

CSV download: https://urlhaus.abuse.ch/downloads/csv_recent/  (last 30 days)
                https://urlhaus.abuse.ch/downloads/csv/         (full database, larger)

Usage:
    python urlhaus.py [--output PATH] [--full] [--force]
"""

from __future__ import annotations

import argparse
import logging
from pathlib import Path

import requests

LOG = logging.getLogger("urlhaus")

RECENT_URL = "https://urlhaus.abuse.ch/downloads/csv_recent/"
FULL_URL = "https://urlhaus.abuse.ch/downloads/csv/"

USER_AGENT = "lupus-security-research/0.1 (+https://github.com/inviti8/lupus)"


def download(url: str, output_path: Path, force: bool = False) -> int:
    """Download the URLhaus CSV. Returns the number of bytes written.

    URLhaus serves the CSV with gzip Content-Encoding, which requests
    transparently decompresses. The result is a plain CSV regardless of
    what we name the output file.
    """
    if output_path.exists() and not force:
        size = output_path.stat().st_size
        LOG.info("Output already exists at %s (%d bytes); use --force to overwrite", output_path, size)
        return size

    output_path.parent.mkdir(parents=True, exist_ok=True)
    LOG.info("Downloading %s", url)

    headers = {"User-Agent": USER_AGENT}
    with requests.get(url, headers=headers, stream=True, timeout=60) as resp:
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
        default=Path(__file__).parent.parent / "raw" / "urlhaus.csv",
        help="Output path (default: ../raw/urlhaus.csv)",
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="Download the full database instead of last 30 days (much larger)",
    )
    parser.add_argument("--force", action="store_true", help="Overwrite existing file")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    url = FULL_URL if args.full else RECENT_URL

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
