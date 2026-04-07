"""Build the unified Lupus security training dataset.

Reads source CSVs from datasets/security/raw/ and produces:
  - examples/train.jsonl
  - examples/eval.jsonl

Sources processed:
  - phishtank.csv     → label=phishing, source=phishtank
  - urlhaus.csv(.zip) → label=malware,  source=urlhaus
  - tranco.csv        → label=safe,     source=tranco

Optional HTML enrichment:
  - tranco_html.jsonl → safe URLs with html_content populated

Usage:
    python build_dataset.py [--eval-split 0.2] [--balance] [--max-per-class N]
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import logging
import random
import zipfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Optional
from urllib.parse import urlparse

from schema import Label, SecurityExample, Source

LOG = logging.getLogger("build_dataset")

REPO_ROOT = Path(__file__).resolve().parents[2]
SECURITY_DIR = REPO_ROOT / "datasets" / "security"
RAW_DIR = SECURITY_DIR / "raw"
EXAMPLES_DIR = SECURITY_DIR / "examples"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def extract_domain(url: str) -> str:
    """Extract the hostname from a URL. Returns lowercased."""
    try:
        parsed = urlparse(url)
        return (parsed.hostname or "").lower()
    except (ValueError, AttributeError):
        return ""


def stable_id(prefix: str, *parts: str) -> str:
    """Build a stable, short ID by hashing the parts together."""
    h = hashlib.sha1("|".join(parts).encode("utf-8")).hexdigest()[:12]
    return f"{prefix}-{h}"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def load_html_index(html_jsonl_path: Optional[Path]) -> dict[str, str]:
    """Load an html_fetcher.py output JSONL into {url: html} dict.

    Skips error records and truncated entries beyond a sane size.
    """
    if html_jsonl_path is None or not html_jsonl_path.exists():
        return {}

    index: dict[str, str] = {}
    with html_jsonl_path.open("r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "error" in rec or "html" not in rec:
                continue
            index[rec["url"]] = rec["html"]
    LOG.info("Loaded HTML for %d URLs from %s", len(index), html_jsonl_path)
    return index


# ---------------------------------------------------------------------------
# Source loaders
# ---------------------------------------------------------------------------


def load_phishtank(csv_path: Path) -> Iterable[SecurityExample]:
    """Yield SecurityExamples from a PhishTank CSV.

    Expected columns: phish_id, url, phish_detail_url, submission_time,
    verified, verification_time, online, target
    """
    if not csv_path.exists():
        LOG.warning("PhishTank source not found at %s", csv_path)
        return

    fetched_at = now_iso()
    with csv_path.open("r", encoding="utf-8", errors="replace") as f:
        reader = csv.DictReader(f)
        for row in reader:
            url = row.get("url", "").strip()
            if not url:
                continue
            domain = extract_domain(url)
            if not domain:
                continue
            verified = row.get("verified", "").lower() == "yes"

            yield SecurityExample(
                id=stable_id("phishtank", url),
                source=Source.PHISHTANK,
                source_id=row.get("phish_id"),
                url=url,
                domain=domain,
                html_content=None,
                html_truncated=False,
                label=Label.PHISHING,
                confidence=95 if verified else 70,
                indicators=[],  # populated later by feature extraction if desired
                target_brand=row.get("target") or None,
                threat_type=None,
                fetched_at=fetched_at,
                verified=verified,
            )


def load_urlhaus(csv_or_zip_path: Path) -> Iterable[SecurityExample]:
    """Yield SecurityExamples from URLhaus CSV (raw or .zip).

    URLhaus CSV starts with comment lines beginning with '#' that must be
    skipped. Columns are: id, dateadded, url, url_status, last_online,
    threat, tags, urlhaus_link, reporter
    """
    if not csv_or_zip_path.exists():
        LOG.warning("URLhaus source not found at %s", csv_or_zip_path)
        return

    fetched_at = now_iso()

    if csv_or_zip_path.suffix == ".zip":
        with zipfile.ZipFile(csv_or_zip_path) as zf:
            csv_name = next((n for n in zf.namelist() if n.endswith(".csv")), None)
            if csv_name is None:
                LOG.warning("URLhaus zip contains no CSV: %s", csv_or_zip_path)
                return
            with zf.open(csv_name) as raw:
                text = raw.read().decode("utf-8", errors="replace")
    else:
        text = csv_or_zip_path.read_text(encoding="utf-8", errors="replace")

    # Skip comment lines starting with #
    data_lines = [line for line in text.splitlines() if line and not line.startswith("#")]
    if not data_lines:
        return

    # First non-comment line is the header
    reader = csv.DictReader(io.StringIO("\n".join(data_lines)))
    for row in reader:
        url = (row.get("url") or "").strip().strip('"')
        if not url:
            continue
        domain = extract_domain(url)
        if not domain:
            continue
        threat = (row.get("threat") or "").strip().strip('"') or None
        tags = (row.get("tags") or "").strip().strip('"')
        indicators = [t.strip() for t in tags.split(",") if t.strip()] if tags else []

        yield SecurityExample(
            id=stable_id("urlhaus", url),
            source=Source.URLHAUS,
            source_id=row.get("id"),
            url=url,
            domain=domain,
            html_content=None,
            html_truncated=False,
            label=Label.MALWARE,
            confidence=95,
            indicators=indicators,
            target_brand=None,
            threat_type=threat,
            fetched_at=fetched_at,
            verified=True,
        )


def load_tranco(csv_path: Path, html_index: dict[str, str], html_max_chars: int) -> Iterable[SecurityExample]:
    """Yield SecurityExamples from a Tranco CSV (rank, domain).

    Tranco gives domains, not URLs — we synthesize https://{domain}/ as the URL.
    If html_index has content for that URL, attach it.
    """
    if not csv_path.exists():
        LOG.warning("Tranco source not found at %s", csv_path)
        return

    fetched_at = now_iso()
    with csv_path.open("r", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        for row in reader:
            domain = (row.get("domain") or "").strip().lower()
            if not domain:
                continue
            rank = int(row.get("rank") or 0)
            url = f"https://{domain}/"

            html = html_index.get(url)
            html_truncated = False
            if html and len(html) > html_max_chars:
                html = html[:html_max_chars]
                html_truncated = True

            yield SecurityExample(
                id=stable_id("tranco", domain),
                source=Source.TRANCO,
                source_id=str(rank),
                url=url,
                domain=domain,
                html_content=html,
                html_truncated=html_truncated,
                # Confidence scaled by rank: top sites are more certain to be safe
                label=Label.SAFE,
                confidence=99 if rank <= 1000 else (95 if rank <= 10000 else 90),
                indicators=[],
                target_brand=None,
                threat_type=None,
                fetched_at=fetched_at,
                verified=True,
            )


# ---------------------------------------------------------------------------
# Build pipeline
# ---------------------------------------------------------------------------


def deduplicate(examples: list[SecurityExample]) -> list[SecurityExample]:
    """Remove duplicate examples by ID, keeping the first occurrence."""
    seen: set[str] = set()
    result: list[SecurityExample] = []
    for ex in examples:
        if ex.id in seen:
            continue
        seen.add(ex.id)
        result.append(ex)
    return result


def balance_classes(
    examples: list[SecurityExample],
    max_per_class: Optional[int] = None,
) -> list[SecurityExample]:
    """Cap each label class to roughly equal size.

    If max_per_class is given, hard-cap each class. Otherwise cap to the
    smallest class size.
    """
    by_label: dict[Label, list[SecurityExample]] = {}
    for ex in examples:
        by_label.setdefault(ex.label, []).append(ex)

    if not by_label:
        return []

    if max_per_class is None:
        max_per_class = min(len(v) for v in by_label.values())

    result: list[SecurityExample] = []
    for label, items in by_label.items():
        random.shuffle(items)
        capped = items[:max_per_class]
        LOG.info("  %s: %d → %d", label.value, len(items), len(capped))
        result.extend(capped)

    random.shuffle(result)
    return result


def split_train_eval(
    examples: list[SecurityExample], eval_fraction: float
) -> tuple[list[SecurityExample], list[SecurityExample]]:
    """Stratified split: each class gets the same eval fraction."""
    by_label: dict[Label, list[SecurityExample]] = {}
    for ex in examples:
        by_label.setdefault(ex.label, []).append(ex)

    train: list[SecurityExample] = []
    eval_: list[SecurityExample] = []
    for items in by_label.values():
        random.shuffle(items)
        cut = int(len(items) * eval_fraction)
        eval_.extend(items[:cut])
        train.extend(items[cut:])

    random.shuffle(train)
    random.shuffle(eval_)
    return train, eval_


def write_jsonl(examples: list[SecurityExample], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for ex in examples:
            f.write(ex.model_dump_json() + "\n")
    LOG.info("Wrote %d examples to %s", len(examples), path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--eval-split",
        type=float,
        default=0.2,
        help="Fraction of examples held out for eval (default: 0.2)",
    )
    parser.add_argument(
        "--balance",
        action="store_true",
        help="Balance class sizes (cap to smallest class or --max-per-class)",
    )
    parser.add_argument(
        "--max-per-class",
        type=int,
        default=None,
        help="Hard cap on examples per class (requires --balance)",
    )
    parser.add_argument(
        "--html-max-chars",
        type=int,
        default=8000,
        help="Truncate HTML content to this many characters (default: 8000)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed for shuffling and splitting",
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    random.seed(args.seed)

    LOG.info("Loading sources from %s", RAW_DIR)
    html_index = load_html_index(RAW_DIR / "tranco_html.jsonl")

    examples: list[SecurityExample] = []
    examples.extend(load_phishtank(RAW_DIR / "phishtank.csv"))
    examples.extend(load_urlhaus(RAW_DIR / "urlhaus.csv.zip"))
    examples.extend(load_urlhaus(RAW_DIR / "urlhaus.csv"))
    examples.extend(load_tranco(RAW_DIR / "tranco.csv", html_index, args.html_max_chars))

    LOG.info("Loaded %d total examples before deduplication", len(examples))
    examples = deduplicate(examples)
    LOG.info("After deduplication: %d", len(examples))

    if not examples:
        LOG.error("No examples loaded. Did you run the fetcher scripts first?")
        LOG.error("  python fetch/phishtank.py")
        LOG.error("  python fetch/urlhaus.py")
        LOG.error("  python fetch/tranco.py")
        return 1

    # Show class distribution
    LOG.info("Class distribution:")
    counts: dict[Label, int] = {}
    for ex in examples:
        counts[ex.label] = counts.get(ex.label, 0) + 1
    for label, count in sorted(counts.items(), key=lambda x: x[0].value):
        LOG.info("  %s: %d", label.value, count)

    if args.balance:
        LOG.info("Balancing classes...")
        examples = balance_classes(examples, max_per_class=args.max_per_class)
        LOG.info("After balancing: %d", len(examples))

    LOG.info("Splitting train/eval (eval_fraction=%.2f)", args.eval_split)
    train, eval_ = split_train_eval(examples, args.eval_split)
    LOG.info("Train: %d  Eval: %d", len(train), len(eval_))

    write_jsonl(train, EXAMPLES_DIR / "train.jsonl")
    write_jsonl(eval_, EXAMPLES_DIR / "eval.jsonl")

    LOG.info("Done. Run `python schema.py` to validate.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
