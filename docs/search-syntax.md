# Browse & Search Syntax

The [Triple Browser](/browse) exposes one consistent search across its Table, Graph and Schema views. There are three complementary tools: the free-text **search box**, the per-field **filter chips**, and the scope-aware **facet rail**. Everything is available at once; the **SPARQL** button in the browser's header shows the query behind whatever you are looking at.

## Free-text search operators

The search box matches a substring across the subject, predicate, object and graph of every triple (case-insensitively, over all pages). It understands a small boolean syntax — operators are written in **UPPERCASE**:

| Example | Meaning |
|---|---|
| `novel poetry` | both words must appear (adjacent terms imply **AND**) |
| `novel AND poetry` | both words (explicit) |
| `novel OR poetry` | either word |
| `novel XOR poetry` | exactly one of the two, not both |
| `novel NOT translation` | `novel` but exclude `translation` |
| `(hardcover OR paperback) novel` | parentheses group sub-expressions |
| `"first edition"` | a quoted phrase is matched whole (spaces kept) |

Precedence, lowest to highest: `OR` → `XOR` → `AND` → `NOT`. Use parentheses when in doubt. Invalid syntax (e.g. unbalanced parentheses) is reported rather than silently ignored.

## Filter chips (per-field)

Chips filter a specific position of the triple. Add several on one field to **OR** them; chips on different fields are **AND**-ed together. Each chip carries a match mode, shown by its glyph and style:

| Mode | Glyph | How to create it |
|---|---|---|
| **contains** (substring) | `≈` | type a value and press Enter — the default |
| **exact** (full IRI / literal) | `=` | pick a value from the autosuggestions, or click a facet |
| **regex** | `.*` | choose the *regex* mode, then type a pattern (case-insensitive) |

## Facets in scope

The rail lists what is actually present in the current dataset / organisation / version scope — **Classes**, **Properties**, **Vocabularies** (grouped by namespace prefix) and **Graphs** (with their detected role) — each with a count. Clicking a facet adds the matching filter chip, so you can drill down without typing IRIs.

## The SPARQL behind the view

The **SPARQL** button opens the query your current scope, chips and search add up to: read it, copy it, or open it in the [SPARQL workspace](/sparql) to take it further. The same panel offers **natural-language search** wherever an LLM gateway is configured — describe what you want in plain English and it drafts the query for you (see [API Services & AI Queries](/docs/api-services)).
