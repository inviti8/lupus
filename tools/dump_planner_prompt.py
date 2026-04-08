#!/usr/bin/env python3
"""Dump the canonical Lupus planner system prompt (with hash + length).

Used by:
  - The daemon's Phase 2 byte-equivalence test (`daemon/tests/prompt_snapshot.rs`)
    embeds the SHA-256 of this prompt as a hardcoded constant. If this script's
    output hash changes, that test fails until the Rust port is updated to
    match. This is the hard guardrail against silent prompt drift between the
    Python eval and the Rust daemon.
  - Manual verification when iterating on the prompt: `python tools/dump_planner_prompt.py | sha256sum`
  - Diffing the rendered prompt against an alternate version to spot
    whitespace bugs: `python tools/dump_planner_prompt.py > /tmp/old.txt`
    then change something and `diff /tmp/old.txt <(python tools/dump_planner_prompt.py)`.

Usage:
    python tools/dump_planner_prompt.py            # full prompt to stdout
    python tools/dump_planner_prompt.py --stats    # just length/hash, no body
    python tools/dump_planner_prompt.py --hash     # just the hash
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tools"))

from tinyagent_prompt_probe import LUPUS_TOOLS, build_planner_system_prompt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stats", action="store_true", help="print length + hash, omit prompt body")
    parser.add_argument("--hash", action="store_true", help="print just the SHA-256 hash")
    args = parser.parse_args()

    prompt = build_planner_system_prompt(LUPUS_TOOLS)
    encoded = prompt.encode("utf-8")
    sha = hashlib.sha256(encoded).hexdigest()

    if args.hash:
        print(sha)
        return 0

    if args.stats:
        print(f"chars:  {len(prompt)}")
        print(f"bytes:  {len(encoded)}")
        print(f"sha256: {sha}")
        return 0

    sys.stdout.write(prompt)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
