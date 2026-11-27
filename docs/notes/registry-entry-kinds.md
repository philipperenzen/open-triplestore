# One kind of registry entry: derived content facets and an optional "standard" flag instead of data-model / vocabulary

> **Status: accepted on 2026-11-25 with the staging of §8 (decisions in §9); step 1 is built and in review (PR #670), steps 2 and 3 are open.** It answers one question: should the model registry stop sorting its entries into *data models* and *vocabularies* (`kind`)? The answer is yes, in three independent steps, keeping `kind` in the API as a derived, deprecated alias (§5). §3 says honestly which of the frictions that prompted this the change does **not** fix. Anchors are `path:line` against `develop` at `28696df2` (0.8.1 plus the unreleased PR #641).

## 1. The question

A registry entry carries one `kind`. Of its five values (`RegistryKind`, `src/kind_detector.rs:105`: `data-model`, `vocabulary`, `shapes`, `entailment`, `instances`) the web UI knows two: `entryKind` files anything that is not `vocabulary` under *data model* (`frontend/src/lib/modelRegistryFilter.ts:29`). Many real entries are both at once. OWL, DCAT and PROV declare classes and properties; a SKOS scheme with dual-typed concepts is a class model and a vocabulary; a quality profile lists dimensions and metrics as individuals of a vocabulary class. Should the registry keep asking which one an entry *is*, or describe what it *holds*?

## 2. What `kind` is today

* **One triple on the entry, not on a version.** `ver:kind` is written by `set_data_model_kind` (`src/data_models/registry.rs:372`) and read back in the three entry queries (`:58`, `:171`, `:268`). It is overwritten on **every** upload: the upload runs `kind_detector::detect` over the parsed quads, an explicit `kind` form field wins, and the verdict is stored (`src/data_models/handlers.rs:1132-1141`, `:1221-1226`). A later version can therefore flip the entry.
* **Guessed from content.** `kind_detector.rs` is 1,660 lines of weighted tallies (`Evidence`, `:166`) behind a five-way verdict, and the rule has just been rewritten (PR #641: any named class means data model unless the scheme is dual-typed).
* **Not even applied consistently inside the product.** The seeder pins every bundled standard and every LOV install to `vocabulary` (`SEEDED_KIND`, `src/data_models/seed_vocab.rs:109`), although detection files OWL, RDFS and DCAT as data models; the code comment says so. A seed bundle declares its own `kind` (`src/seed_bundles/manifest.rs:405-419`).
* **Storage and API are already one registry.** Both kinds live in one registry graph, are listed by `GET /api/models`, and have the same version graph IRI `{base}/data-model/{id}/version/{v}` (`docs/models.md:99`). A client that wants "vocabularies" filters the one list by `kind`.
* **Little behaviour hangs on it.** Besides the badge and the filter chips (`frontend/src/pages/ModelRegistry.svelte:204-213`, `:271-273`): the fallback of version stamping when a graph has neither an ontology nor a scheme header (`src/data_models/version_iri.rs:289`); the DCAT asset type `CodeList` against `Ontology` (`src/catalog/builder.rs:116`); the public catalogue's two sections (`src/catalog/public.rs:181`); the commit-log kind (`src/data_models/handlers.rs:2644`, `:2974`); the kind a dataset's graphs get when a dataset role is promoted into the registry (`src/auth/handlers.rs:6075-6079`); the `kind` string of platform entries in vocabulary search (`src/vocab_search/catalog.rs:609`). Thirteen Rust files name `RegistryKind`; only the places above make a decision with it.
* **The facets already exist.** The detector's `Evidence` counts classes, properties, shapes, SKOS schemes, concepts and relations, rules and typed subjects in the same single pass the upload already makes, and the upload response already returns it (`detected`, `mixed`, `evidence`, `src/data_models/handlers.rs:1259-1264`). The dataset page derives the same signals from the browse facets (`frontend/src/lib/content-kind.ts:26-34`). Only the verdict is stored.

## 3. Frictions, and what a merge does about each

| # | Friction (seen loading real standards as seed bundles) | Caused by `kind`? | Fixed by one entry type + facets | Fixed on its own |
|---|---|---|---|---|
| F1 | A SKOS concept scheme in a **dataset** raises "Datasets should hold instance data … Convert to Vocabulary" (`frontend/src/components/ContentKindWarning.svelte`, `computeMismatch`, verdict from `classifyVerdict`, `content-kind.ts`). A thesaurus kept as data gets nagged, so it ends up in a registry entry only. | Yes: the banner is the split applied to datasets. | Yes: concept schemes become a facet, not a verdict; the banner is dropped for them (§6.4). | Partly: suppress the banner for SKOS-only content. |
| F2 | A vocabulary made only of **individuals** (quality dimensions and metrics) shows 0 classes and 0 properties and the banner "instance data in a model registry", which reads as an empty entry. | Yes: the entry page counts only classes and properties, and `instance-in-model` fires for `instances` and `mixed` verdicts, whatever the entry was registered as. | Yes: an *individuals* count next to the others; the finding is reported once, where the content is added, not each time a version is opened (§6.4). | Yes, the same two edits. |
| F3 | Vocabulary search defaults to classes and properties (`src/vocab_search/routes.rs:350-353`, `frontend/src/pages/VocabularySearch.svelte`), so concepts and individuals look missing until the Instance filter is on. | No: a default in the search engine and its page. | No. | Yes: the page selects all term types; the API's LOV-compatible default stays. |
| F4 | A graph a registry version names can never be a dataset's graph (`graph_held_by_model_registry`, `src/auth/dataset_graph.rs:69`), so "the model inside the dataset" needs a second copy of every graph under another IRI. | **No.** The rule is an ownership and licence boundary: a dataset import must not overwrite a version past its licence checks, expose withheld versions, or wipe it on dataset delete (`dataset_graph.rs:366-380`). It holds for every kind. | **No.** | No; needs its own mechanism (§6.6). |
| F5 | The kind is a guess, and the guess has just been changed (PR #641): the same file uploaded before and after can be filed differently. | Yes. | Yes: stored kinds are never recomputed; new entries derive an alias from facets. | No. |

F4 is the friction people feel most and the merge does not touch it. The proposal should not be sold as fixing it.

## 4. Options

**A. Fix F1-F3 one by one, keep `kind`.** Cheapest (a day or two), no API change. Leaves F5, the inconsistent seeder override, and a registry page that still asks users to pick between two labels that overlap.

**B. Rename to "Standards".** Rejected. Many entries are drafts, third-party vocabularies (`xsd`, `omg`), internal models, or shapes-only. A name that claims normative status for all of them is wrong for most.

**C. One entry type; content facets derived from the data; an optional "standard" flag; `kind` kept as a derived, deprecated alias.** Removes the question instead of answering it better, and gives F1 and F2 a principled fix (the facets) rather than two more special cases. Costs more: an API addition, a registry page redesign, and a deprecation to carry.

## 5. Recommendation

**C, shipped as three separate upstream PRs, each useful without the next** (§8). The deciding facts are in §2: storage and API are already one registry, the behaviour hanging on `kind` is half a dozen call sites, the detector already computes the facets, and the seeder already overrides the guess. What is left of the split is a label that is wrong in both directions. Step 1 alone is option A with the facets as the way of fixing F1-F3; steps 2 and 3 can be dropped without wasting it.

## 6. Design of C

### 6.1 Content facets

Per **version**, counts of: `classes`, `properties`, `concepts` (with `concept_schemes`), `individuals`, `shapes`, `rules`. Each is the detector's own tally (`Evidence`: `owl_classes + rdfs_classes`, `owl_properties`, `skos_concepts`, `skos_concept_schemes`, `instance_subjects`, `shacl_shapes`, `entailment_rules`), so no new classification logic and no new thresholds.

* **Where computed.** On upload, from the scan the upload already makes; on seeding, by scanning the version's graphs once loaded. Stored on the version record as one JSON value, the way the attribution record is (`ver:attribution`, `src/data_models/registry.rs:1217`), with the number of triples it was counted over. A stored record is served only while the content still holds that many triples, so a version edited since is counted again when opened; a change that keeps the count the same waits for the next start.
* **Existing versions.** A background pass on start counts versions with no facets or stale ones (content above two million triples is reported `capped`, with no counts); `GET` of one version counts it inline up to 250,000 triples. The list serves what is stored. No stored `kind` is touched.
* **API.** `GET /api/models` entries gain `facets` (the latest published version's, or latest draft's when none is published); version objects gain `facets`. Additive: no field is renamed or removed.

### 6.2 The optional "standard" flag

An entry may carry `standard: { issuing_body, url }`, absent by default. It is set by an owner or admin (`PATCH /api/models/{id}`) or by a seed bundle (`[data_models.standard]`), and shown as a badge and a filter. It is curation, not detection: nothing infers it from content or from a licence record. A normative status ("mandatory", "recommended") is left out until someone needs to filter on it, because its values are a policy choice of the operator (§9, decision 2).

### 6.3 `kind` as a derived, deprecated alias

* A stored `ver:kind` stays exactly as it is and is returned as before. The migration rewrites nothing.
* **Upload stops writing the detected verdict to the entry.** Only an explicit choice is stored: the upload's `kind` field, a manifest's `kind`, and the seeder's `SEEDED_KIND`. This ends the flip-flop between versions.
* An entry with no stored kind gets `kind` derived from its facets by the rule `Evidence` already encodes (`kind_detector.rs`: classes mean `data-model` unless a dual-typed concept scheme dominates; otherwise concepts or properties mean `vocabulary`; shapes only, `shapes`; rules only, `entailment`; individuals only, `instances`), so a client that reads `kind` keeps working and sees the same values it would have seen.
* OpenAPI marks `kind` deprecated; `?kind=` filters, `/models?kind=vocabulary` deep links, the upload `kind` field and the manifest `kind` keep working. Removal is not proposed before 1.0.

### 6.4 UI

* **Registry page.** The Models / Vocabularies chips become facet chips (Classes, Properties, Concepts, Individuals, Shapes) plus a "Standard" toggle; the one badge becomes the entry's facet chips. `/models?kind=…` still pre-selects the equivalent chip.
* **Entry and version pages.** The statistics show every non-zero facet. An entry of 140 concepts says "140 concepts", not "0 classes, 0 properties".
* **Banners (`ContentKindWarning.svelte`, `content-kind.ts`).** Today the banner is mounted in a version's inline browser (`frontend/src/pages/ModelDetail.svelte:729`, always `expected="model"`, whatever the entry is) and in the model viewer (`ModelViewer.svelte:76`), so it repeats on every visit to an entry that was filed on purpose. It goes: the entry pages show chips instead, and a version's row shows its chips as soon as it is uploaded (the upload response already carries the evidence). In a dataset, SKOS-only content raises no banner; classes, properties, shapes and rules in a dataset keep their note and its *Convert* action. The probe and the cheap facet path keep classifying identically, as the comment at `content-kind.ts:26` requires.
* **Dataset conformance.** `conforms_to_model` already accepts any entry and the conformance layer follows the version's graphs without looking at `kind` (`src/conformance.rs:150`); the picker lists entries with their chips and sorts "Standard" first.
* **Vocabulary search.** The search page selects all four term types by default; the API keeps the LOV-compatible default `class,property` (F3).

### 6.5 Migration and compatibility

| Item | Change |
|---|---|
| Stored entries | None. `ver:kind` stays; facets and `standard` are new triples, backfilled lazily. |
| REST | Additive (`facets`, `standard`); `kind` deprecated, not removed. |
| URLs and IRIs | Unchanged; `{base}/data-model/{id}/version/{v}` already serves both. |
| Seed bundles | `kind` still accepted; `[data_models.standard]` optional. |
| DCAT and public catalogue | The ADMS asset type (`dct:type`) stays `CodeList` when the alias is `vocabulary`, `Ontology` otherwise: same output as today. The two public sections become one list with chips (PR 2). |
| Docs | `docs/models.md`, `docs/linked-data-modelling-styleguide.md` §2.3, the role-detection page. |

### 6.6 What this leaves open: F4

A dataset that wants "the model inside it" is asking to *include* a registry version, not to copy it. The conformance layer already resolves a dataset's model version to its graphs (`src/conformance.rs:150-170`). The natural follow-up is a read-only *included* reference in the dataset's graph list (browse, query and export see it; writes, graph delete and role changes do not), so no second copy is needed and the ownership boundary is unchanged. It needs its own proposal; nothing here blocks or depends on it.

## 7. Risks

* **Facet cost on large entries.** Counting is a scan. Mitigations: computed at upload from quads already in memory; backfill is background and capped; the list endpoint reads stored values only.
* **Deprecation drag.** Clients keep reading `kind` for a long time. Accepted: the alias is cheap to keep.
* **Derived alias disagrees with a stored kind** for the same content (seeded standards: stored `vocabulary`, derived `data-model`). Intended: the stored value wins; the facets tell the truth about content.
* **Review surface.** The registry page and its tests change in step 2; steps 1 and 3 are small.

## 8. Staging

1. **PR 1: facets and the specific frictions.** Facet computation and storage, `facets` in the API, entry-page statistics, banner placement and rules (F1, F2), vocabulary-search default (F3). `kind` untouched. Tests: facet tallies from `Evidence` fixtures (including an individuals-only vocabulary and a SKOS-only scheme), upload and seed record facets, lazy backfill of a version without them, banner verdicts in vitest.
2. **PR 2: `kind` becomes an alias; the page follows.** Upload stops storing the detected verdict; derived alias; facet chips and filters; public catalogue list; docs; deprecation in OpenAPI and `CHANGELOG.md`. Tests: a stored kind survives a later upload of other content; derived alias table; `?kind=` and the deep link.
3. **PR 3: the `standard` flag.** Stored, `PATCH`, manifest key, badge, filter, conformance-picker ordering.

Separately, and not waiting for any of it: a seed bundle's multi-graph version was recorded with its first graph as base graph and left out of the sub-graphs, so that graph was not served, hashed or deleted. That is a bug fix with its own PR.

## 9. Decisions

The maintainer answered the five questions this note first asked (2026-11-25):

1. **Name.** The UI says **Registry**, not "Model & Vocabulary Registry". "Standards" stays out as the name of an entity (§4 B); the `standard` badge carries it.
2. **Standard status.** `issuing_body` plus `url` is enough. There is no normative status until an operator needs to filter on it; its values are that operator's policy.
3. **Banners.** The SKOS-only banner is dropped for datasets. A dataset that holds classes, properties, shapes or rules keeps its note and its *Convert* action.
4. **Deprecation window.** `kind` is not removed before 1.0.
5. **Staging.** Three PRs as in §8. The seeder fix stays its own PR.
