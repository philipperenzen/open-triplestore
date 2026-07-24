# Vocabulary Search & Prefix Service

Open Triplestore ships its own vocabulary lookup service — an internal
replacement for [Linked Open Vocabularies (LOV)](https://lov.linkeddata.es/)
and [prefix.cc](https://prefix.cc/), both of which are frequently unreachable.
Everything works offline: the data is bundled with the platform.

## What's included

- **Vocabulary catalog** — metadata for ~900 vocabularies from the LOV corpus
  (titles, descriptions, tags, version history, reuse metrics), overlaid with
  the models and vocabularies registered on this instance.
- **Term search** — search classes, properties, datatypes and instances across
  every catalog vocabulary *and* every public vocabulary registered here.
- **Recommender** — give it the field names of a dataset and get back a
  minimal set of vocabularies that covers them all.
- **Offline install** — copy any LOV vocabulary into the model registry with
  one call, no network needed.
- **Prefix service** — ~3,700 prefix ↔ namespace mappings (full prefix.cc
  snapshot + LOV), used by SPARQL auto-prefixing, the editor autocomplete and
  a public lookup API.

Open the UI at **Manage → Vocabulary Search** (`/vocabularies`).

## Term search

`GET /api/vocab/terms/search?q=person&type=class,property`

Parameters follow the LOV API v2: `q`, `type` (comma-separated:
`class,property,datatype,instance`; default `class,property`), `vocab`
(restrict to one prefix), `tag`, `page`, `page_size`, plus the OTS extension
`source` (`platform` | `lov`). The response envelope carries
`total_results`, `results` and `aggregations` (type / vocabulary / tag facet
counts).

Ranking follows LOV's formula: BM25 text relevance over boosted fields
(local name ^12, labels ^3, comments/descriptions ^1.5, parent-vocabulary
text ^1) blended with square-root-dampened popularity — LOD-corpus reuse
metrics where the LOV dump provides them, and **local usage**: terms already
used in this instance's datasets rank higher. Popularity is normalized
against the current result set, so text relevance always dominates.

Also available:

- `GET /api/vocab/terms/autocomplete?q=foaf:Pe` — typeahead over prefixed
  names and local names.
- `GET /api/vocab/terms/suggest?q=persn` — "did you mean" fuzzy suggestions.

Term search requires the `vocab-search` build feature (part of `full`, i.e.
enabled in the standard Docker image); without it these endpoints answer 503
while the catalog and prefix endpoints keep working.

## Vocabulary catalog

- `GET /api/vocab/list` — all vocabularies (platform entries first).
- `GET /api/vocab/search?q=sensor&tag=IoT` — ranked vocabulary search.
- `GET /api/vocab/info?vocab=foaf` — one full record; accepts a prefix,
  ontology URI or namespace.
- `GET /api/vocab/tags` — tag cloud.
- `GET /api/vocab/status` — corpus/index health.

Catalog entries carry `source` (`platform` | `lov`), `model_id` when the
vocabulary is registered on this instance, and `installable` when its full
content is present in the bundled corpus.

## Recommender

`POST /api/vocab/recommend`

```json
{
  "terms": [
    { "term": "person", "category": "class" },
    { "term": "family name", "category": "property" }
  ],
  "preferred_vocabs": { "schema": 0.2 }
}
```

A port of the CLARIAH
[vocabulary-recommender](https://github.com/CLARIAH/vocabulary-recommender):
per-term results are min-max normalized, then the **combiSQORE** algorithm
selects a minimal set of vocabularies that covers every term, biased toward
high-scoring and preferred vocabularies. The response lists the homogeneous
vocabulary set plus, per term, the best match inside that set and the full
ranked alternatives.

## Installing a vocabulary (admin)

`POST /api/vocab/install` with `{"vocab": "gr"}` copies the GoodRelations
vocabulary from the bundled corpus into the model registry as a public,
published entry — exactly like the bundled standard vocabularies, with the
LOV version label and provenance notes. The new entry immediately joins term
search, prefix resolution and the DCAT catalog.

## Prefix service

- `GET /api/prefixes?q=foa` — ranked search.
- `GET /api/prefixes/foaf` — forward lookup; comma multi-lookup
  (`/api/prefixes/rdf,foaf,dcat`) mirrors prefix.cc.
- `GET /api/prefixes/reverse?uri=http://xmlns.com/foaf/0.1/` — reverse lookup
  (term IRIs resolve via longest-namespace matching).
- `GET /api/prefixes/expand?curie=foaf:name` / `GET /api/prefixes/shrink?iri=…`
- `GET /api/prefixes/all?format=json|jsonld|ttl|sparql|csv|txt` — bulk export.
- `GET /api/prefixes/context.jsonld` — a JSON-LD `@context` of every mapping.

Resolution order everywhere (including SPARQL auto-prefixing): vocabularies
registered on this instance → bundled snapshot → previously confirmed cache.
Live prefix.cc is only contacted when the operator sets
`PREFIX_CC_FALLBACK=true`.

## The LOV corpus

Term search over the full LOV corpus and offline installs need the ~18 MB
`lov.nq.gz` dump. The Docker image ships it baked in; otherwise the server
downloads it once at boot (sha256-verified) into `{data_dir}/vocab/`.

| Variable | Default | Meaning |
|---|---|---|
| `VOCAB_CORPUS_PATH` | unset | Explicit path to a `lov.nq.gz` file |
| `VOCAB_CORPUS_URL` | pinned archive snapshot | Download URL; `""` disables the download |
| `VOCAB_CORPUS_SHA256` | pinned digest | Expected checksum for a custom URL |
| `VOCAB_LOCAL_METRICS` | on | `off` skips the local-usage ranking signal |
| `PREFIX_CC_FALLBACK` | off | `true` re-enables live prefix.cc fallback |

Without the corpus the service degrades gracefully: vocabulary/prefix search
and the catalog stay fully functional; term search covers the vocabularies
registered on this instance (45+ ship with the standard seed).

The term index is built once per corpus snapshot in a boot background task
(`{data_dir}/vocab_index/`) and reopened instantly on later boots.

## Attribution

Vocabulary metadata and contents derive from the
[Linked Open Vocabularies](https://lov.linkeddata.es/) dataset (CC BY 4.0,
Vandenbussche et al.). Prefix mappings derive from the community-maintained
[prefix.cc](https://prefix.cc/) registry. See `NOTICE` for details.
