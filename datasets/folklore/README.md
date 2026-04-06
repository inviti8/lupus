# Hare & Wolf Folklore Compendium

Training dataset of folk tales and mythologies surrounding the Hare and the Wolf from world traditions. Gives Lupus genuine cultural depth rooted in the traditions that inspired its name.

## Why

Lupus (wolf constellation) and Lepus (hare constellation) draw on mythology going back millennia. The Anishinaabe tradition of Ma'iingan (Wolf) as Nanabozho's (Great Hare) companion and pathfinder is the project's spiritual foundation. Training on this folklore means the model understands these traditions at depth — not as decoration, but as knowledge.

## Structure

Each entry is a JSON object:

```json
{
  "tradition": "Anishinaabe",
  "title": "Nanabozho and Ma'iingan",
  "type": "creation_myth",
  "characters": ["Nanabozho (Great Hare)", "Ma'iingan (Wolf)"],
  "summary": "...",
  "full_text": "...",
  "themes": ["companionship", "pathfinding", "naming"],
  "source": "Basil Johnston, Ojibway Heritage",
  "license": "public_domain"
}
```

## Traditions Covered

- **Anishinaabe / Ojibwe** — Nanabozho and Ma'iingan (primary inspiration)
- **Pan-Algonquian** — Michabo / Great Hare, wolf clan traditions
- **Japanese** — Tsuki no Usagi (moon rabbit), Ōkami (wolf deity)
- **Chinese** — Jade Rabbit (Yùtù), Turkic/Mongolian wolf origins
- **Korean** — Dal-tokki (moon rabbit), founding myths
- **Mesoamerican** — Rabbit in the Moon (Tecciztecatl)
- **West African** — Zomo, Kalulu, Sungura (hare tricksters)
- **Greco-Roman / European** — Lepus constellation, Romulus & Remus, Fenrir
- **Celtic / Irish** — Shape-shifting hare, Wolves of Ossory
- **Russian / Slavic** — Grey Wolf (Серый Волк), Zaichik
- **Indian / South Asian** — Śaśajātaka (self-sacrificing hare), Panchatantra
- **Native American (Plains)** — Rabbit trickster, Wolf as spirit guide
- **Arctic / Inuit** — Arctic hare, Amarok (giant wolf)
- **Egyptian** — Wenet (hare goddess), Wepwawet (opener of ways)
- **Australian Aboriginal** — Hare-wallaby Dreaming
- **Aesop & fable traditions** — Tortoise and the Hare, Boy Who Cried Wolf

## Thematic Threads

Cross-tradition patterns that connect the stories:

1. **Hare as trickster-creator** — clever, fast, reshapes the world through wit
2. **Wolf as pathfinder-guardian** — loyal, guides through danger, opens the way
3. **The Wolf-Hare bond** — they walk together; the wolf scouts ahead for the hare
4. **Moon associations** — the hare lives in the moon across cultures
5. **Constellation myths** — Lupus and Lepus adjacent in the southern sky

## Sources

Priority order:
1. Public domain texts (pre-1928, Project Gutenberg, Sacred Texts Archive, Internet Archive)
2. Published collections by Indigenous and tradition-bearing authors
3. Academic folklore journals and anthropological records
4. Synthetic structuring of known tales via LLM (for format consistency)

## Ethics

- Prioritize sources shared publicly by their own communities
- Do not scrape sacred or restricted knowledge
- Use collections by Indigenous authors where possible
- When in doubt about a story's cultural status, omit it
- Credit traditions and sources, never present as generic "mythology"

## Target

500–1,000 structured entries + 100–200 cross-tradition thematic syntheses.

## Directory

```
folklore/
  tales/          ← Individual tale entries (JSON)
  themes/         ← Cross-tradition thematic syntheses
  sources.bib     ← Bibliography of source materials
```
