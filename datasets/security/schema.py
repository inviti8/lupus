"""Pydantic schemas for the Lupus security training dataset.

Single source of truth for SecurityExample records. Every line in
datasets/security/examples/*.jsonl validates against these models.

Run `python schema.py` to validate the entire datasets/security directory.
"""

from __future__ import annotations

import json
from enum import Enum
from pathlib import Path
from typing import Any, Optional

from pydantic import BaseModel, Field, ValidationError


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------


class Label(str, Enum):
    """The classification label for a security example."""
    SAFE = "safe"
    PHISHING = "phishing"
    MALWARE = "malware"
    SUSPICIOUS = "suspicious"


class Source(str, Enum):
    """The data source the example came from."""
    PHISHTANK = "phishtank"
    OPENPHISH = "openphish"
    URLHAUS = "urlhaus"
    TRANCO = "tranco"
    MANUAL = "manual"


# ---------------------------------------------------------------------------
# SecurityExample
# ---------------------------------------------------------------------------


class SecurityExample(BaseModel):
    """A single labeled security training example."""

    id: str = Field(..., description="Unique identifier (typically source-prefixed)")
    source: Source = Field(..., description="Data source the example came from")
    source_id: Optional[str] = Field(None, description="Original ID in the source database")

    url: str = Field(..., description="The URL being classified")
    domain: str = Field(..., description="Extracted hostname (eTLD+1)")

    html_content: Optional[str] = Field(
        None,
        description="Raw HTML body, possibly truncated. None for URL-only examples.",
    )
    html_truncated: bool = Field(
        False,
        description="True if html_content was cut at the size limit",
    )

    label: Label = Field(..., description="Classification label")
    confidence: int = Field(..., ge=0, le=100, description="Confidence in label, 0-100")

    indicators: list[str] = Field(
        default_factory=list,
        description="Threat indicators detected (e.g. lookalike_domain, credential_form)",
    )
    target_brand: Optional[str] = Field(
        None,
        description="For phishing: what brand is being impersonated",
    )
    threat_type: Optional[str] = Field(
        None,
        description="For malware: type of threat (e.g. trojan, ransomware, drive-by)",
    )

    fetched_at: str = Field(..., description="ISO 8601 timestamp")
    verified: bool = Field(False, description="Whether the source verified this entry")

    def to_training_text(self, html_max_chars: int = 8000) -> str:
        """Format this example as a training prompt for Qwen2.5-Coder.

        The model learns: (URL + HTML) → structured JSON analysis matching
        the daemon's ScanResponse format.
        """
        analysis = {
            "label": self.label.value,
            "score": self._compute_score(),
            "indicators": self.indicators,
        }
        if self.target_brand:
            analysis["target_brand"] = self.target_brand
        if self.threat_type:
            analysis["threat_type"] = self.threat_type

        html_section = ""
        if self.html_content is not None:
            html = self.html_content
            if len(html) > html_max_chars:
                html = html[:html_max_chars] + "...[truncated]"
            html_section = f"\n### HTML:\n{html}"

        return (
            f"### URL: {self.url}"
            f"{html_section}\n"
            f"### Analysis:\n"
            f"{json.dumps(analysis, separators=(',', ':'))}"
        )

    def _compute_score(self) -> int:
        """Compute the trust score (0-100) from the label.

        100 = fully safe, 0 = critical threat.
        """
        if self.label == Label.SAFE:
            return self.confidence
        # For threats, invert: high confidence in threat = low trust score
        return 100 - self.confidence


# ---------------------------------------------------------------------------
# Validation helpers
# ---------------------------------------------------------------------------


def validate_examples_dir(security_dir: Path) -> tuple[int, list[str]]:
    """Validate every line in every datasets/security/examples/*.jsonl file.

    Returns (count, errors).
    """
    errors: list[str] = []
    count = 0
    examples_dir = security_dir / "examples"
    if not examples_dir.exists():
        return 0, [f"{examples_dir} does not exist"]

    for path in sorted(examples_dir.glob("*.jsonl")):
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
                SecurityExample.model_validate(data)
                count += 1
            except (json.JSONDecodeError, ValidationError) as e:
                errors.append(f"{path}:{line_no}: {e}")
    return count, errors


def main() -> int:
    repo_root = Path(__file__).resolve().parents[2]
    security_dir = repo_root / "datasets" / "security"

    print(f"Validating {security_dir}...")
    count, errors = validate_examples_dir(security_dir)
    print(f"  {count} examples validated")
    for err in errors:
        print(f"  ERROR: {err}")

    if errors:
        return 1
    print("All entries valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
