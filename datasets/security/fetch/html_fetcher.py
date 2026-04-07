"""Safe HTML fetcher for Tranco (legitimate) URLs.

Fetches HTML body content for a list of URLs with strict safety controls.
ONLY use this for known-safe URLs (Tranco list, manually curated allowlists).
NEVER use this for phishing or malware URLs — see ../README.md for the
rationale and the cultural rules.

Safety controls enforced:
- 10s connect/read timeout
- Max 1 MB response body (truncated if larger)
- Custom User-Agent identifying as research crawler
- No JavaScript execution (we use requests, not a browser)
- Max 3 redirects
- Content-Type whitelist (text/html only)
- Courteous rate limiting (1 req/sec by default, configurable)
- Retries on transient errors only (429, 503), not on 4xx
- Per-domain concurrency limit (don't hammer single hosts)

Output: JSONL with one record per URL containing the fetched HTML and
metadata, or an error record if the fetch failed.

Usage:
    python html_fetcher.py --input ../raw/tranco.csv --output ../raw/tranco_html.jsonl
    python html_fetcher.py --input ../raw/tranco.csv --output ../raw/tranco_html.jsonl --limit 1000
"""

from __future__ import annotations

import argparse
import csv
import json
import logging
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional
from urllib.parse import urlparse

import requests
from requests.adapters import HTTPAdapter
from urllib3.util.retry import Retry

LOG = logging.getLogger("html_fetcher")

USER_AGENT = "lupus-security-research/0.1 (+https://github.com/inviti8/lupus)"

DEFAULT_TIMEOUT = 10  # seconds
DEFAULT_MAX_BYTES = 1_000_000  # 1 MB
DEFAULT_MAX_REDIRECTS = 3
DEFAULT_RATE_LIMIT_SEC = 1.0  # min seconds between requests
ALLOWED_CONTENT_TYPES = ("text/html", "application/xhtml+xml")


def make_session() -> requests.Session:
    """Build a requests session with sensible defaults and retry policy."""
    session = requests.Session()
    session.headers.update({
        "User-Agent": USER_AGENT,
        "Accept": "text/html,application/xhtml+xml",
        "Accept-Language": "en-US,en;q=0.5",
    })
    session.max_redirects = DEFAULT_MAX_REDIRECTS

    # Retry only on transient errors (429, 503), not on 4xx
    retry_strategy = Retry(
        total=2,
        backoff_factor=2,
        status_forcelist=[429, 503],
        allowed_methods=["GET", "HEAD"],
    )
    adapter = HTTPAdapter(max_retries=retry_strategy)
    session.mount("http://", adapter)
    session.mount("https://", adapter)
    return session


def fetch_one(
    session: requests.Session,
    url: str,
    timeout: int = DEFAULT_TIMEOUT,
    max_bytes: int = DEFAULT_MAX_BYTES,
) -> dict:
    """Fetch a single URL with all safety controls. Returns a result dict.

    Result dict has either:
      - {"url": ..., "html": ..., "status_code": ..., "truncated": bool, "fetched_at": ...}
      - {"url": ..., "error": ..., "fetched_at": ...}
    """
    fetched_at = datetime.now(timezone.utc).isoformat()
    try:
        resp = session.get(url, timeout=timeout, stream=True, allow_redirects=True)

        # Check content type before downloading body
        content_type = resp.headers.get("Content-Type", "").lower()
        if not any(ct in content_type for ct in ALLOWED_CONTENT_TYPES):
            return {
                "url": url,
                "error": f"unsupported content-type: {content_type}",
                "status_code": resp.status_code,
                "fetched_at": fetched_at,
            }

        if resp.status_code != 200:
            return {
                "url": url,
                "error": f"http {resp.status_code}",
                "status_code": resp.status_code,
                "fetched_at": fetched_at,
            }

        # Stream-read with byte limit
        body = bytearray()
        truncated = False
        for chunk in resp.iter_content(chunk_size=8192, decode_unicode=False):
            body.extend(chunk)
            if len(body) >= max_bytes:
                truncated = True
                break
        resp.close()

        # Decode with whatever encoding the server claims, fallback to utf-8 with replacement
        encoding = resp.encoding or "utf-8"
        try:
            html = bytes(body[:max_bytes]).decode(encoding, errors="replace")
        except (LookupError, UnicodeDecodeError):
            html = bytes(body[:max_bytes]).decode("utf-8", errors="replace")

        return {
            "url": url,
            "final_url": resp.url,
            "html": html,
            "status_code": resp.status_code,
            "truncated": truncated,
            "bytes": len(body),
            "fetched_at": fetched_at,
        }

    except requests.Timeout:
        return {"url": url, "error": "timeout", "fetched_at": fetched_at}
    except requests.TooManyRedirects:
        return {"url": url, "error": "too_many_redirects", "fetched_at": fetched_at}
    except requests.SSLError as e:
        return {"url": url, "error": f"ssl_error: {e}", "fetched_at": fetched_at}
    except requests.ConnectionError as e:
        return {"url": url, "error": f"connection_error: {e}", "fetched_at": fetched_at}
    except requests.RequestException as e:
        return {"url": url, "error": f"request_error: {e}", "fetched_at": fetched_at}


def read_input_urls(input_path: Path, url_column: Optional[str] = None) -> list[str]:
    """Read URLs from a CSV file.

    If the file has a 'url' column, use it. Otherwise if it has a 'domain'
    column (Tranco format), prepend 'https://' to each domain.
    """
    urls: list[str] = []
    with input_path.open("r", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        if reader.fieldnames is None:
            raise ValueError(f"{input_path} has no header row")

        if url_column and url_column in reader.fieldnames:
            for row in reader:
                urls.append(row[url_column])
        elif "url" in reader.fieldnames:
            for row in reader:
                urls.append(row["url"])
        elif "domain" in reader.fieldnames:
            for row in reader:
                urls.append(f"https://{row['domain']}")
        else:
            raise ValueError(
                f"{input_path} has no 'url' or 'domain' column "
                f"(found: {reader.fieldnames})"
            )

    return urls


def already_fetched_urls(output_path: Path) -> set[str]:
    """Return URLs already present in the output JSONL (for resumability)."""
    seen: set[str] = set()
    if not output_path.exists():
        return seen
    with output_path.open("r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
                if "url" in record:
                    seen.add(record["url"])
            except json.JSONDecodeError:
                continue
    return seen


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input",
        type=Path,
        required=True,
        help="Input CSV with 'url' or 'domain' column (e.g. ../raw/tranco.csv)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Output JSONL path",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=0,
        help="Maximum URLs to fetch this run (0 = no limit)",
    )
    parser.add_argument(
        "--rate-limit",
        type=float,
        default=DEFAULT_RATE_LIMIT_SEC,
        help=f"Min seconds between requests (default: {DEFAULT_RATE_LIMIT_SEC})",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=DEFAULT_TIMEOUT,
        help=f"Per-request timeout in seconds (default: {DEFAULT_TIMEOUT})",
    )
    parser.add_argument(
        "--max-bytes",
        type=int,
        default=DEFAULT_MAX_BYTES,
        help=f"Max response body bytes (default: {DEFAULT_MAX_BYTES})",
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )

    LOG.info("Reading URLs from %s", args.input)
    urls = read_input_urls(args.input)
    LOG.info("Found %d URLs", len(urls))

    seen = already_fetched_urls(args.output)
    if seen:
        LOG.info("Skipping %d URLs already fetched in %s", len(seen), args.output)

    todo = [u for u in urls if u not in seen]
    if args.limit > 0:
        todo = todo[: args.limit]

    LOG.info("Fetching %d URLs", len(todo))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    session = make_session()

    success = 0
    failure = 0
    last_request_time = 0.0

    with args.output.open("a", encoding="utf-8") as out_f:
        for i, url in enumerate(todo, start=1):
            # Rate limit
            elapsed = time.monotonic() - last_request_time
            if elapsed < args.rate_limit:
                time.sleep(args.rate_limit - elapsed)
            last_request_time = time.monotonic()

            result = fetch_one(
                session,
                url,
                timeout=args.timeout,
                max_bytes=args.max_bytes,
            )
            out_f.write(json.dumps(result, ensure_ascii=False) + "\n")
            out_f.flush()  # safe against interruption

            if "error" in result:
                failure += 1
                LOG.debug("[%d/%d] FAIL %s: %s", i, len(todo), url, result["error"])
            else:
                success += 1
                LOG.debug("[%d/%d] OK   %s (%d bytes)", i, len(todo), url, result.get("bytes", 0))

            if i % 100 == 0:
                LOG.info("Progress: %d/%d (%d ok, %d fail)", i, len(todo), success, failure)

    LOG.info("Done. %d ok, %d fail. Total in output: %d", success, failure, len(seen) + len(todo))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
