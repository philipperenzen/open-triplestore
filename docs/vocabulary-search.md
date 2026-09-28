# Vocabulary Search & Prefix Service

Open Triplestore ships its own vocabulary lookup service — an internal
replacement for [Linked Open Vocabularies (LOV)](https://lov.linkeddata.es/)
and [prefix.cc](https://prefix.cc/), both of which are frequently unreachable.
Everything works offline: the data is bundled with the platform.

## What's included

- **Vocabulary catalog** — metadata for ~900 vocabularies from the LOV corpus
  (titles, tags, version history, reuse metrics, and each vocabulary's
  licence; descriptions where that licence allows), overlaid with the models
  and vocabularies registered on this instance.
- **Term search** — search classes, properties, datatypes and instances across
  the LOV vocabularies in this instance's corpus that we may redistribute
  (the Docker image ships those, 517 of the 898; see *The LOV corpus*) *and*
  every public vocabulary registered here.
- **Recommender** — give it the field names of a dataset and get back a
  minimal set of vocabularies that covers them all.
- **Offline install** — copy a LOV vocabulary from the local corpus (the
  vocabularies this instance holds) into the model registry with one call,
  no network needed.
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
- `GET /api/vocab/notice?vocab=foaf` — the same vocabulary's licence page, as
  plain text: its licences with their URIs, where they are stated, the notice
  copies must carry, whether we redistribute it, where LOV's copy comes from
  and LOV's own attribution. The licence record of every installed LOV
  vocabulary links here.
- `GET /api/vocab/tags` — tag cloud.
- `GET /api/vocab/status` — corpus/index health.

Catalog entries carry `source` (`platform` | `lov`), `model_id` when the
vocabulary is registered on this instance, and `installable` when the corpus
on this instance holds the vocabulary's triples (see *The LOV corpus*).

LOV entries also carry the vocabulary's licence:

| Field | Meaning |
|---|---|
| `license` | The recognised licences, e.g. `["CC BY 3.0"]` |
| `license_uris` | Each licence's URI, in the same order as `license` (`null` for a label that names no licence document, such as a publisher's public-domain statement) |
| `license_declared` | The licence and rights statements exactly as the vocabulary's own graph gives them |
| `license_status` | `open`, `restricted` (NonCommercial, all rights reserved, copyleft such as GPL or GFDL), `unrecognised`, `copyright-only` or `none` |
| `redistributable` | Whether we may redistribute it: the status is `open` (the licence allows sharing an unmodified copy with attribution) and nothing in `redistribution_withheld` |
| `redistribution_withheld` | Why an `open` vocabulary is still not redistributed, or `null` (see below) |
| `license_source` | `graph`: the licence is what the vocabulary's own graph states. `publisher-terms`: the graph names none, and the licence is what its publisher states elsewhere (see below) |
| `license_source_url` | For `publisher-terms`: the publisher's statement of its terms |
| `license_notice` | The statement the licence requires on copies, whatever its source, or `null` when it asks only for the usual attribution (see *Notices* below) |
| `no_derivatives` | Every licence offered allows only unaltered copies (CC BY-ND, the OGC Document Notice): an installed copy cannot be edited or copied into a draft (see *Installing a vocabulary*) |
| `lov_misdecoded` | How many literals of LOV's copy hold characters LOV mis-decoded, in a graph we redistribute anyway because its licence allows modification; `0` otherwise (see *Notices*) |

The licence normally comes from the vocabulary's own graph. Many publishers
state their terms elsewhere instead: W3C puts the documents on its site under
its Document License, DCMI licenses its schemas under CC BY 4.0, the FOAF and
schema.org specifications give their licences in the page footer, ESCO states
its reuse terms in its FAQ. When a graph names no licence (none, or only a
copyright line), the catalog uses those terms from a reviewed table in
`scripts/build_lov_catalog.py` (`PUBLISHER_TERMS`). Every row cites a primary
source from the rights holder, and `license_source` says the licence came
from there. A licence the graph itself states always wins, including a
restrictive one. W3C's Document License is applied only to namespaces W3C
serves from `www.w3.org` (http and https), and OGC's Document Notice to
`www.opengis.net`. Both let anyone copy and distribute a document's contents
in any medium, with the notices below, but allow no modified versions (W3C's
only in software).

**Notices.** `license_notice` holds what the licence asks copies to carry:

- W3C Document License: the copyright notice in the form the licence gives,
  with the year of the document (its own date, else LOV's date for the
  version it holds), the status where the graph states one (e.g. "Working
  Draft 29 April 2011"), and, because LOV re-serialized the document, the
  notice the licence gives for material "copied from or derived from" it,
  with its title and IRI. A document that carries its own copyright notice
  keeps that one.
- OGC Document Notice: the notice of the original file (for `sf` and `gml`,
  "Copyright (c) 2012 Open Geospatial Consortium." with its status and
  version), which LOV's copy drops.
- MIT and BSD: the rights holder's copyright line, quoted from its licence
  file (`scripts/build_lov_catalog.py`, `GRAPH_NOTICES`).
- Publisher statements: DCMI's schema notice, ESCO's "This service uses the
  ESCO classification of the European Commission.", and so on.
- ISA Open Metadata Licence (LOCN, Person, RADion, OSLO, and the ISA-derived
  material in ADMS and RegOrg): the copyright notice and the licence's "No
  Warranty" disclaimer, which it requires every distribution to keep.
- UK Open Government Licence (reegle): its default attribution statement,
  since the provider gives none; the Flemish Modellicentie (the Flemish
  Person vocabulary): the licensor's name and year, as its article 4 asks.
- Mis-decoded text: where LOV decoded UTF-8 as Latin-1 or Windows-1252 in a
  graph whose licence allows modification, the notice says that LOV's copy,
  which ships as LOV distributes it, differs from the publisher's document in
  those literals (for the graphs checked against the publisher's own file:
  that LOV introduced the errors). Under a licence that allows no
  modification such a graph is withheld instead.

**Withheld vocabularies.** Some `open` vocabularies are still not
redistributed, and `redistribution_withheld` says why: LOV's copy of a work
whose licence allows no modification is not faithful (drammar, CC BY-ND, and
W3C's Profiles Vocabulary, in which LOV mis-decoded some characters), or the
licence requires a copyright notice that neither the vocabulary nor its
rights holder states (Inrupt's Authentication Provider vocabulary, MIT), or
the licence is not identified well enough to meet its conditions (the
Translational Medicine Ontology names "Creative Commons - By Attribution"
without a version, and every version asks copies to carry the URI of the one
that applies).

A vocabulary's description is included only when `redistributable` is true.
Many of LOV's descriptions are copied from the vocabularies themselves, and
that text stays under the vocabulary's licence, not LOV's. Where the licence
allows no modification (CC BY-ND, the W3C and OGC document licences) the
description is kept only if it is character for character a literal of the
vocabulary's own graph. Titles, tags, dates, versions, reuse metrics and
creator names are kept for every vocabulary. Platform entries have
`license_status: null`: their terms are those of their registry entry.

The `source` block of `/api/vocab/list` and `/api/vocab/status` describes the
catalog itself. Its `license` (CC BY 4.0, `license_url`) is LOV's licence for
LOV's own metadata only, as `license_scope` says; `modifications` says what
this project changed.

## Recommender

`POST /api/vocab/recommend` — public, no auth required; also driven by the
**Recommender** tab on the Vocabularies page.

```json
{
  "terms": [
    { "term": "person", "category": "class" },
    { "term": "family name", "category": "property" }
  ],
  "preferred_vocabs": { "schema": 0.2 }
}
```

Terms also accept plain strings for the common case (`category` defaults to
`all`):

```bash
curl -s -X POST "$BASE/api/vocab/recommend" \
  -H "Content-Type: application/json" \
  -d '{"terms": ["bridge", "deck height"]}'
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
vocabulary from the local corpus into the model registry as a public,
published entry — exactly like the bundled standard vocabularies, with the
LOV version label and provenance notes. The new entry immediately joins term
search, prefix resolution and the DCAT catalog.

The version notes and the response (`license`, `license_status`,
`license_source`, `license_source_url`, `license_notice`) record the
vocabulary's licence — CC BY 3.0 for GoodRelations, from its own graph — not
LOV's CC BY 4.0, which does not cover it, and the notice that licence
requires. The vocabulary's own licence and copyright statements come along
with its triples, which are loaded verbatim: nothing is added to the graph. A
vocabulary the local corpus does not hold answers `503`; with the Docker
image's corpus that means we may not redistribute it (see below).

The installed version also gets a **licence record** (`attribution` on
`/api/models/{id}/versions`), like the bundled vocabularies: its licences
with their URIs, the required notice, the source, whether the store holds
LOV's copy unchanged (checked after the load) and a link to
`/api/vocab/notice`. The check compares with LOV's copy as the store holds
it: the store writes some typed literals in its canonical form, with the same
values (`"1"^^xsd:nonNegativeInteger` as `"1"^^xsd:integer`, a `+00:00` time
zone as `Z`), and the record says so when that applies to the vocabulary. Downloads of the version carry the licences in their
`Link` headers. A vocabulary whose every licence allows only unaltered copies
(`no_derivatives`: CC BY-ND, the OGC Document Notice) gets
`no_derivatives: true` in that record, so the registry refuses to copy it
into a draft or branch or otherwise edit it (`403`): an edited copy would be
an altered one.

**Vocabularies we may not redistribute install as private.** The image's
corpus holds only redistributable vocabularies, but an operator who mounts
the full dump (`VOCAB_CORPUS_PATH`, see *The LOV corpus*) can install any of
them, including NonCommercial, GPL and unlicensed ones. A public entry would
re-serve such a vocabulary to anyone. So a vocabulary we may not
redistribute (`redistributable: false`: `restricted`, `unrecognised`,
`copyright-only` or `none`, or withheld) is installed as a **private**
registry entry,
owned by the installing admin: only that owner and the admins see it, and it
stays out of term search, the prefix overlay and the public catalog. The
response says so (`is_public: false`, and `note`), and so do the version
notes. An admin who has checked the vocabulary's terms can make the entry
public afterwards (`PATCH /api/models/{id}` with `{"is_public": true}`).
Redistributable vocabularies install public, as before.

**Installs by earlier releases.** Releases before these rules installed every
vocabulary public, added an `owl:versionInfo` triple to graphs that state no
version of their own, and noted LOV's CC BY 4.0 as the licence ("Installed
from the bundled LOV corpus (snapshot 2025-12-18, CC BY 4.0)."). Every boot
checks for such installs, before term search and the catalog see the
registry. A version counts as one only when everything that installer did
holds, so that a user's own model is never taken for one:

- its note is exactly that sentence (every earlier release shipped the
  2025-12-18 catalogue; an edited note is left to an admin, and logged when
  the entry is public and holds a vocabulary we may not serve publicly);
- its entry has no owner (the installer made none; every entry a user
  creates has one, and only admins may write an entry without one), and the
  entry's id and namespace are a LOV vocabulary's prefix and namespace;
- it is a published version whose graph is the conventional
  `{base}/data-model/{id}/version/{version}` graph, not derived from another
  version or on a branch;
- the entry and the version name the same creator: none (an install made
  with sign-in off), or a user of this instance who, if the instance still
  knows them, is an admin (only admins could install).

For each such install:

- when we may not redistribute the vocabulary, or its licence allows only
  unaltered copies (the graph may hold the added triple, and nothing has
  checked it since), the entry is made **private**: admins only, with no
  owner assigned. This happens once; the registry marks the version, and an
  admin who has checked the vocabulary's terms can make the entry public
  again (`PATCH /api/models/{id}`), which later boots leave alone. The server
  logs each entry it makes private;
- the version gets a licence record naming the vocabulary's own licence and
  notice, with `unchanged: false`: it says the earlier release may have added
  an `owl:versionInfo` triple and that the graph may have been modified. For
  a no-derivatives vocabulary, the registry's no-derivatives guards then
  apply to the entry: no drafts, branches or edits, and its copy, not shown
  to be unaltered, is served only to those who may write the entry, even if
  an admin makes the entry public again. An admin who deletes the entry and
  installs the vocabulary again (from a corpus that holds it) gets a checked,
  unchanged copy. A licence
  record the migration did not write is left as it is.

Nothing else changes: no triple is removed or added, and the version notes
stay as they are. The check needs no LOV corpus, runs only where the store
may be written (not on a replication follower, or on a Raft member that does
not lead; a member that gains leadership runs it within 30 s), and once
nothing is left to do it costs one registry query per boot. A follower
notices registry changes that arrive by replication (the leader made an entry
private, say) within 30 s, and its next vocab request rebuilds term search and
the catalog from them. Installs whose note was cleared cannot be told from
other models and are not checked.

## Prefix service

- `GET /api/prefixes?q=foa` — ranked search.
- `GET /api/prefixes/foaf` — forward lookup; comma multi-lookup
  (`/api/prefixes/rdf,foaf,dcat`) mirrors prefix.cc.
- `GET /api/prefixes/reverse?uri=http://xmlns.com/foaf/0.1/` — reverse lookup
  (term IRIs resolve via longest-namespace matching).
- `GET /api/prefixes/expand?curie=foaf:name` / `GET /api/prefixes/shrink?iri=…`
- `GET /api/prefixes/all?format=json|jsonld|ttl|sparql|csv|txt` — bulk export.
- `GET /api/prefixes/context.jsonld` — a JSON-LD `@context` of every mapping.

Resolution order everywhere (including SPARQL auto-prefixing): **administrator
overrides** → vocabularies registered on this instance → prefixes declared by
installed seed bundles → bundled snapshot → previously confirmed cache. Live
prefix.cc is only contacted when the operator sets `PREFIX_CC_FALLBACK=true`.

### Where the bundled prefixes come from

The bundled snapshot (`src/prefixes/data/prefixes-snapshot.json`, built by
`scripts/build_prefix_dataset.py`) merges two third-party sources. Its
`sources` block records where each came from and on what terms:

| Source | Entries | Taken from | Terms |
|---|---|---|---|
| [prefix.cc](https://prefix.cc/) | 3,537 | `https://prefix.cc/popular/all.file.json`, 2026-06-29 | No licence is published for the data. The pairs are facts, and the operator has stated that all data is considered public domain (CC0) in [cygri/prefix.cc#13](https://github.com/cygri/prefix.cc/issues/13), though no licence or dedication has been published. The Unlicense on the [site's repository](https://github.com/cygri/prefix.cc) covers its code, not the data. |
| [Linked Open Vocabularies (LOV)](https://lov.linkeddata.es/) | 157 | the LOV dump of 2025-12-18 | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Modified: an extract of the valid prefix/namespace pairs whose label prefix.cc does not already bind (157 of LOV's 898 vocabularies). |

prefix.cc is maintained by Richard Cyganiak and was developed at DERI, NUI
Galway. LOV was created by Pierre-Yves Vandenbussche and Bernard Vatant and is
hosted by the Ontology Engineering Group, Universidad Politécnica de Madrid.
prefix.cc's popularity order is kept as each entry's rank; LOV entries rank
after it.

Every deployment re-serves the snapshot through `/api/prefixes/all` and
`/api/prefixes/context.jsonld`. The Turtle and SPARQL exports open with `#`
comment lines crediting both sources, and the CSV export has a `source` column.
The JSON, JSON-LD and plain-text exports carry no credit, because anything
added to them would be read as a prefix. If you republish one of those, give
the LOV credit with it: the name, a link to CC BY 4.0, and that the pairs are
an extract.

### Saying what a prefix means here

A community list is a good default and a poor authority: `geo` means one thing
on prefix.cc and quite another on a deployment that publishes its own geo
namespace. An administrator can say which:

| | | |
|---|---|---|
| `GET` | `/api/admin/prefixes` | the overrides this deployment has set |
| `POST` | `/api/admin/prefixes` | claim a shorthand — `{"label": "geo", "namespace": "https://data.example.org/geo/def/"}` |
| `PUT` | `/api/admin/prefixes/{label}` | set or repoint one |
| `DELETE` | `/api/admin/prefixes/{label}` | drop the override |

Admin-only, because repointing a prefix changes what every stored CURIE
expands to.

**No two overrides share a shorthand.** The label is the primary key of the
table they live in, so this is a property of the storage rather than a check
somebody has to remember to run: `POST` of a label that already has one is
refused with `409` and told what it currently resolves to. Repointing is a
`PUT`, which is a different request on purpose — a prefix changing meaning
should be a decision, not a side effect. Two *labels* may share a namespace
(`dct` and `dcterms` are both right), so the constraint is on the label alone.

Deleting an override does not delete the prefix. It drops this deployment's
opinion of it, and the label falls back to whichever lower tier answers first.

Overrides are stored in the identity database, so they survive a restart and
reach a follower with the rest of it. The in-memory overlay is reloaded on
every write and at boot, so a running process never disagrees with what is
stored. An entry that fails validation on the way in — a label that does not
start with a letter, a namespace that is not an `http(s)` IRI — is refused with
a reason; one already in a database restored from elsewhere is dropped with a
warning rather than trusted.

## The LOV corpus

Term search over LOV vocabularies and offline installs need a `lov.nq.gz`
N-Quads dump: one named graph per vocabulary, plus LOV's metadata graph, which
the server does not read (the catalog is compiled in).

**What the Docker image ships: only the vocabularies we may redistribute.**
LOV publishes its own data under CC BY 4.0, but the vocabulary graphs in its
dump are their publishers' works, each under the licence its publisher chose.
Many declare none, and some declare one that does not allow redistribution
(NonCommercial, all rights reserved, GPL and similar copyleft). So the image
build downloads the pinned 2025-12-18 dump, checks its sha256 and keeps only
the graphs listed in `assets/vocab/lov-redistributable.txt`: the vocabularies
whose licence allows sharing an unmodified copy with attribution (CC0 and
public-domain marks, CC BY, BY-SA and BY-ND, ODC, Apache-2.0, MIT, BSD, W3C,
OGC, Etalab and similar open-government licences) — the licence the graph
declares or, where it declares none, the terms its publisher states elsewhere
(see *Vocabulary catalog* above) — less the withheld ones. Each graph ships as
LOV's N-Quads serialization of it, as LOV's dump holds it, unchanged: the
triples, with the licence and copyright statements among them; comments in
the publisher's source file are not part of it, which is why the notices are
listed separately. In 14 graphs LOV mis-decoded characters (see *Notices*);
their notice says so.
Everything else is dropped, LOV's metadata graph included. The list — graph
IRI, LOV prefix, licence, the licence's source (`graph`, or the URL of the
publisher's terms) and the notice the licence requires, tab-separated, one
graph per line — ships next to the corpus as
`/app/assets/vocab/lov-redistributable.txt`. `scripts/build_lov_catalog.py`
generates it along with the catalog, byte for byte the same from the same
dump.

In that snapshot 517 of the 898 vocabularies qualify (516 have a graph in the
dump): 431 by the licence in their own graph and 86 by their publisher's
terms — among them RDF, RDFS, OWL, SKOS, PROV and the other W3C namespaces
(W3C Document License), ODRL (W3C Software and Document License), Dublin Core
(CC BY 4.0), FOAF (CC BY 1.0), schema.org (CC BY-SA 3.0), the RDA element
sets, the NEPOMUK ontologies, ESCO and OMG. Four more have an open licence
but are withheld (see *Vocabulary catalog*). Of the rest, 49 are restrictive
(one of them, FAO's geopolitical ontology, under its publisher's
non-commercial terms), 9 declare a licence the script does not recognise, 15
give only a copyright line and 304 state no licence anywhere we found.
Vocabularies registered on the instance, the standard seed included, stay
term-searchable either way.

Many of those licences ask for a statement on copies. Each is in the entry's
`license_notice` and in the last column of `lov-redistributable.txt`; the root
`NOTICE` points to them for the image's corpus. The licence texts that must
travel with copies are in `LICENSES/`.

**Using the full dump.** An operator may fetch the complete dump from LOV's
public archive for their own instance. The server uses the first corpus it
finds — `VOCAB_CORPUS_PATH`, then `{data_dir}/vocab/lov.nq.gz`, then the
image's filtered copy — and without any it downloads the pinned full dump
once at boot (sha256-verified) into `{data_dir}/vocab/`. With the Docker image,
put the full dump in the data volume and restart:

```bash
docker exec <container> curl -fL -o /data/vocab/lov.nq.gz \
  https://web.archive.org/web/20251218081818id_/https://lov.linkeddata.es/lov.nq.gz
```

or build the image with `--build-arg LOV_CORPUS_URL=` so no filtered copy is
baked in. The vocabularies this adds come under their own terms, which the
catalog reports per entry. Those we may not redistribute install as private
registry entries (see *Installing a vocabulary*), and they stay out of term
search and the recommender: the term index serves labels and definitions to
anyone, so it takes only the redistributable vocabularies, whatever the
corpus holds. `GET /api/vocab/status` reports `corpus_vocabularies` (how many
catalog vocabularies the corpus in use holds) and, in `engine`,
`lov_vocabularies` (how many of them are term-indexed).

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
(`{data_dir}/vocab_index/`) and reopened instantly on later boots; a release
whose catalog changes which vocabularies may be redistributed rebuilds it.
Once the new index is in use, the boot removes the earlier ones (another
corpus, catalogue or index format), which may hold vocabularies this release
does not serve.

## Attribution

The vocabulary catalog is derived from the metadata of
[Linked Open Vocabularies](https://lov.linkeddata.es/) (LOV), created by
Pierre-Yves Vandenbussche and Bernard Vatant and hosted by the Ontology
Engineering Group, Universidad Politécnica de Madrid, under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). It is modified:
extracted from the dump of 2025-12-18 and converted to JSON, with licence
fields, link counts and term metrics added and the descriptions of
vocabularies we do not redistribute removed. The image's corpus is an
extract of the same dump (the redistributable vocabularies' graphs, as LOV's
dump holds them, unchanged; where LOV mis-decoded characters, the graph's
notice says so). Each vocabulary, and any text quoted from it, stays under its
publisher's licence, reported per entry (with where the licence is stated
and the notice it requires) and in `lov-redistributable.txt`. Prefix
mappings derive from the community-maintained
[prefix.cc](https://prefix.cc/) registry, maintained by Richard Cyganiak and
developed at DERI, NUI Galway (no licence is published for its data; its
operator has stated the data is considered public domain, CC0), and, for 157
prefixes prefix.cc lacks, from an extract of LOV (CC BY 4.0). See *Where the
bundled prefixes come from* above and `NOTICE` for details.

Term search and autocomplete index a registry entry that holds content whose
licence allows no altered copies only while its latest published version is a
checked, unchanged copy — the same rule the version downloads apply to
anonymous callers.
