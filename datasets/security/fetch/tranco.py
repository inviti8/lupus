"""Tranco fetcher — downloads the Tranco Top 1M list.

Tranco is a research-focused ranked list of popular websites combining
Alexa, Cisco Umbrella, Majestic, and Quantcast rankings. It's the standard
'safe URL' source in academic security research.

The list is generated daily and stable list IDs are published. We download
the most recent stable list by ID.

Usage:
    python tranco.py [--output PATH] [--top N] [--list-id ID] [--force]
"""

from __future__ import annotations

import argparse
import csv
import logging
import zipfile
from io import BytesIO
from pathlib import Path

import requests

LOG = logging.getLogger("tranco")

# The 'latest' endpoint redirects to the most recent stable list
LATEST_URL = "https://tranco-list.eu/top-1m.csv.zip"
LIST_BY_ID_URL = "https://tranco-list.eu/download/{list_id}/full"

USER_AGENT = "lupus-security-research/0.1 (+https://github.com/inviti8/lupus)"


def download_zip(url: str) -> bytes:
    LOG.info("Downloading %s", url)
    headers = {"User-Agent": USER_AGENT}
    resp = requests.get(url, headers=headers, timeout=60)
    resp.raise_for_status()
    return resp.content


def extract_top_n(zip_bytes: bytes, top_n: int) -> list[tuple[int, str]]:
    """Extract the top N entries from a Tranco ZIP. Returns [(rank, domain), ...]."""
    rows: list[tuple[int, str]] = []
    with zipfile.ZipFile(BytesIO(zip_bytes)) as zf:
        # Tranco zips contain a single CSV with the same name minus .zip
        csv_name = next(name for name in zf.namelist() if name.endswith(".csv"))
        with zf.open(csv_name) as f:
            text = f.read().decode("utf-8")
            reader = csv.reader(text.splitlines())
            for row in reader:
                if len(row) < 2:
                    continue
                try:
                    rank = int(row[0])
                except ValueError:
                    continue
                domain = row[1].strip()
                rows.append((rank, domain))
                if len(rows) >= top_n:
                    break
    LOG.info("Extracted %d entries from Tranco list", len(rows))
    return rows


def write_csv(rows: list[tuple[int, str]], output_path: Path) -> None:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with output_path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["rank", "domain"])
        for rank, domain in rows:
            writer.writerow([rank, domain])
    LOG.info("Wrote %d rows to %s", len(rows), output_path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).parent.parent / "raw" / "tranco.csv",
        help="Output path (default: ../raw/tranco.csv)",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=50000,
        help="Number of top sites to extract (default: 50000)",
    )
    parser.add_argument(
        "--list-id",
        help="Specific Tranco list ID to download (default: latest stable list)",
    )
    parser.add_argument("--force", action="store_true", help="Overwrite existing file")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    if args.output.exists() and not args.force:
        LOG.info("Output already exists at %s; use --force to overwrite", args.output)
        return 0

    url = LIST_BY_ID_URL.format(list_id=args.list_id) if args.list_id else LATEST_URL

    try:
        zip_bytes = download_zip(url)
        rows = extract_top_n(zip_bytes, args.top)
        write_csv(rows, args.output)
    except requests.HTTPError as e:
        LOG.error("HTTP error: %s", e)
        return 1
    except requests.RequestException as e:
        LOG.error("Network error: %s", e)
        return 1
    except (zipfile.BadZipFile, StopIteration) as e:
        LOG.error("Failed to parse Tranco zip: %s", e)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
