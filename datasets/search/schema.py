"""Pydantic schemas for the Lupus search adapter dataset and folklore compendium.

Single source of truth for both the cultural compendium (FolkloreTale) and the
derived training examples (SearchExample). Every JSON file in datasets/folklore/
and every line in datasets/search/examples/*.jsonl validates against these.

Run `python schema.py` to validate the entire datasets directory.
"""

from __future__ import annotations

import json
from enum import Enum
from pathlib import Path
from typing import Any, Optional

from pydantic import BaseModel, Field, ValidationError


# ---------------------------------------------------------------------------
# Compendium types — datasets/folklore/tales/
# ---------------------------------------------------------------------------


class TaleType(str, Enum):
    CREATION_MYTH = "creation_myth"
    TRICKSTER_TALE = "trickster_tale"
    HERO_TALE = "hero_tale"
    ORIGIN_MYTH = "origin_myth"
    FABLE = "fable"
    MORAL_TALE = "moral_tale"
    CULTURE_HERO = "culture_hero"
    TRANSFORMATION = "transformation"


class License(str, Enum):
    PUBLIC_DOMAIN = "public_domain"
    CC = "cc"
    INDIGENOUS_PUBLISHED = "indigenous_published"  # Published by tradition-bearer for sharing
    UNKNOWN = "unknown"


class Character(BaseModel):
    """A character in a folklore tale."""
    name: str = Field(..., description="Name in original or anglicized form")
    role: str = Field(..., description="Brief role description, e.g. 'Great Hare, trickster-creator'")


class SourceReference(BaseModel):
    """Bibliographic source for a tale."""
    citation: str = Field(..., description="Full bibliographic citation")
    indigenous_author: bool = Field(
        False,
        description="True if the author is from the tradition the tale belongs to",
    )
    url: Optional[str] = Field(None, description="Link to source if publicly available")


class FolkloreTale(BaseModel):
    """A culturally curated folklore entry in the compendium."""

    id: str = Field(..., description="Unique slug, e.g. 'anishinaabe-nanabozho-and-maiingan'")
    tradition: str = Field(..., description="Cultural tradition name")
    title: str = Field(..., description="Title in English (with original-language form if relevant)")
    type: TaleType = Field(..., description="Tale category")
    characters: list[Character] = Field(..., min_length=1)
    summary: str = Field(..., description="1-2 sentence summary")
    full_text: str = Field(
        ...,
        description="200-400 word respectful retelling preserving cultural authenticity",
    )
    themes: list[str] = Field(..., min_length=1, description="Thematic tags")
    moral: Optional[str] = Field(None, description="The wisdom of the tale, if applicable")
    source: SourceReference
    cultural_notes: Optional[str] = Field(
        None,
        description="Cultural context, sensitivities, or significance notes",
    )
    license: License


# ---------------------------------------------------------------------------
# Training example types — datasets/search/examples/
# ---------------------------------------------------------------------------


class ExampleCategory(str, Enum):
    TOOL_CALLING = "tool_calling"
    MULTI_STEP = "multi_step"
    KNOWLEDGE_AWARE = "knowledge_aware"
    NO_TOOL = "no_tool"


class ExpectedToolCall(BaseModel):
    """A tool call the model is expected to emit, used for evaluation."""
    tool: str = Field(..., description="Tool name from daemon/src/tools/")
    arguments: dict[str, Any] = Field(..., description="Tool arguments")


class SearchExample(BaseModel):
    """A training example for the search adapter."""

    id: str = Field(..., description="Unique identifier")
    category: ExampleCategory
    user_query: str = Field(..., description="What the user asks")
    assistant_response: str = Field(
        ...,
        description=(
            "The response. May contain TinyAgent function call markers: "
            "<|function_call|>{...}<|end_function_call|>"
        ),
    )
    expected_tool_calls: list[ExpectedToolCall] = Field(
        default_factory=list,
        description="Tool calls expected in the response (for evaluation)",
    )
    metadata: dict[str, Any] = Field(
        default_factory=dict,
        description="Category-specific tags (tradition, themes, source_tale_id, etc.)",
    )


# ---------------------------------------------------------------------------
# Validation helpers
# ---------------------------------------------------------------------------


def validate_folklore_dir(folklore_dir: Path) -> tuple[int, list[str]]:
    """Validate every JSON file under datasets/folklore/tales/.

    Returns (count, errors).
    """
    errors: list[str] = []
    count = 0
    tales_dir = folklore_dir / "tales"
    if not tales_dir.exists():
        return 0, [f"{tales_dir} does not exist"]

    for path in sorted(tales_dir.rglob("*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            FolkloreTale.model_validate(data)
            count += 1
        except (json.JSONDecodeError, ValidationError) as e:
            errors.append(f"{path}: {e}")
    return count, errors


def validate_examples_dir(search_dir: Path) -> tuple[int, list[str]]:
    """Validate every line in every datasets/search/examples/*.jsonl file.

    Returns (count, errors).
    """
    errors: list[str] = []
    count = 0
    examples_dir = search_dir / "examples"
    if not examples_dir.exists():
        return 0, [f"{examples_dir} does not exist"]

    for path in sorted(examples_dir.glob("*.jsonl")):
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
                SearchExample.model_validate(data)
                count += 1
            except (json.JSONDecodeError, ValidationError) as e:
                errors.append(f"{path}:{line_no}: {e}")
    return count, errors


def main() -> int:
    repo_root = Path(__file__).resolve().parents[2]
    folklore_dir = repo_root / "datasets" / "folklore"
    search_dir = repo_root / "datasets" / "search"

    print(f"Validating {folklore_dir}...")
    folklore_count, folklore_errors = validate_folklore_dir(folklore_dir)
    print(f"  {folklore_count} tales validated")
    for err in folklore_errors:
        print(f"  ERROR: {err}")

    print(f"Validating {search_dir}...")
    example_count, example_errors = validate_examples_dir(search_dir)
    print(f"  {example_count} examples validated")
    for err in example_errors:
        print(f"  ERROR: {err}")

    if folklore_errors or example_errors:
        return 1
    print("All entries valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
