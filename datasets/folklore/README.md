# Hare & Wolf Folklore Compendium

A curated compendium of folk tales and mythologies from world traditions, centered on the cultural figures of the Hare and the Wolf. This is the **cultural source material** for the Lupus search adapter — a permanent reference that exists alongside (and feeds into) the training data.

## Why this exists

Lupus (wolf constellation) and Lepus (hare constellation) draw on mythology going back millennia. The Anishinaabe tradition of Ma'iingan (Wolf) as Nanabozho's (Great Hare) companion and pathfinder is the project's spiritual foundation. The compendium gives Lupus genuine cultural depth — not as decoration, but as knowledge the model carries.

## Two artifacts, one source of truth

```
datasets/
  folklore/                       ← THE COMPENDIUM (this directory)
    tales/                        ← FolkloreTale entries (the cultural source)
      anishinaabe/
      japanese/
      egyptian/
      ...
  search/                         ← THE TRAINING DATA
    examples/
      knowledge_aware.jsonl       ← SearchExamples DERIVED from compendium tales
```

The compendium is **culturally curated source material** — structured tale entries with full text, characters, themes, sources, and cultural notes. It is valuable in its own right, beyond ML training. Each tale entry is a JSON file matching the `FolkloreTale` schema (`datasets/search/schema.py`).

The training examples in `datasets/search/examples/knowledge_aware.jsonl` are **derived** from the compendium by a conversion script. The model trains on the derived examples; the compendium remains the source of truth.

This separation means we can re-derive training examples with different prompting strategies as the methodology evolves, without losing the cultural curation work.

## Structure

```
folklore/
  README.md
  tales/
    anishinaabe/
      nanabozho-and-maiingan.json
      ...
    japanese/
      tsuki-no-usagi.json
      ...
    egyptian/
      wepwawet-opener-of-ways.json
      ...
    ...
```

Each tale lives in `tales/<tradition>/<slug>.json` and validates against the `FolkloreTale` Pydantic model.

## Schema

See `datasets/search/schema.py` for the canonical Pydantic definitions. A `FolkloreTale` includes:

- `id` — unique slug (e.g., `anishinaabe-nanabozho-and-maiingan`)
- `tradition` — cultural tradition name
- `title` — title in English (with original-language form if relevant)
- `type` — tale category (`creation_myth`, `trickster_tale`, `hero_tale`, `origin_myth`, `fable`, `moral_tale`)
- `characters` — list of characters with name + role
- `summary` — 1-2 sentence summary
- `full_text` — 200-400 word respectful retelling
- `themes` — thematic tags (companionship, trickster, transformation, pathfinding, etc.)
- `moral` — the wisdom of the tale, if applicable
- `source` — bibliographic citation, with flag for indigenous_author
- `cultural_notes` — context, sensitivities, significance
- `license` — `public_domain`, `cc`, `indigenous_published`, or `unknown`

## Traditions covered

The compendium aims for breadth across world cultures, with depth where the wolf-and-hare bond is most central. Priority traditions:

| Tier | Tradition | Why central |
|------|-----------|-------------|
| **Core** | **Anishinaabe / Ojibwe** | The project's spiritual foundation — Nanabozho and Ma'iingan |
| Tier 1 | Japanese | Tsuki no Usagi (moon hare), Ōkami (wolf deity) |
| Tier 1 | Egyptian | Wepwawet (wolf opener-of-ways), Wenet (hare goddess) |
| Tier 1 | Russian / Slavic | Grey Wolf as magical helper, hare as messenger |
| Tier 1 | Aesop / fable tradition | Tortoise & Hare, Wolf in Sheep's Clothing, etc. |
| Tier 2 | Pan-Algonquian | Michabo and wolf clan traditions |
| Tier 2 | Chinese / Mongolian | Jade Rabbit, Turkic wolf origin myths |
| Tier 2 | Indian / South Asian | Śaśajātaka (self-sacrificing hare), Panchatantra wolves |
| Tier 2 | Celtic / Irish | Shape-shifting hare, Wolves of Ossory |
| Tier 2 | Native American (Plains) | Rabbit trickster, wolf as spirit guide |
| Tier 3 | Korean, Mesoamerican, West African, Inuit, Australian Aboriginal | Coverage breadth |

**Target:** 100-200 high-quality `FolkloreTale` entries — 20-25 in the Anishinaabe core, 8-12 each in Tier 1 traditions, 5-8 each in Tier 2, 2-5 each in Tier 3.

## Cultural ethics

This is sacred and meaningful material to many communities. Rules we follow:

1. **Use published sources by tradition-bearing authors first.** Basil Johnston (Ojibwe) for Anishinaabe stories. Erdoes & Ortiz for collected Native American myths. Sources cited per entry.
2. **Public domain or explicitly published-for-sharing only.** Pre-1928 texts, Sacred Texts Archive, Internet Archive collections.
3. **No sacred or restricted material.** Some stories are seasonal, ceremonial, or restricted to specific contexts. When in doubt, omit. The compendium is not a complete record of any tradition — it is a respectful selection.
4. **Cite the tradition by its own name** wherever possible (Anishinaabe, not "Native American"; Ojibwe, not generic; specific Indigenous nation when known).
5. **Note cultural sensitivities** in the `cultural_notes` field of each entry.
6. **Open to correction.** If a community member identifies an entry as inappropriate to share publicly, it gets removed from the compendium and the derived training examples are regenerated.

## Generation method

Entries are generated directly in Claude Code sessions, validated against the Pydantic schema, and committed to git. This uses the maintainer's Claude Code subscription rather than API calls — see `docs/TRAINING_STRATEGY.md` §2.5 for the rationale.

## Conversion to training examples

A future script (`datasets/search/derive_from_folklore.py`) will:
1. Walk `datasets/folklore/tales/`
2. For each `FolkloreTale`, generate 5-15 different `SearchExample` entries (different query angles: direct knowledge query, comparative query, character-focused query, theme-focused query, etc.)
3. Append them to `datasets/search/examples/knowledge_aware.jsonl`

Until that script exists, derived examples can be generated alongside the compendium entries directly.
