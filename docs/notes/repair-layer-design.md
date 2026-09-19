All evidence is in hand: every "wrong" item is confirmed against the file, and I have the citations needed for the design amendments (persistence off the store, aggregate sandbox budget, content-derived proposal id, replacement-rule stratification, per-triple provenance placement, corpus share). Here is the corrected note.

# Repair layer design: TGD/EGD rules, chase-based canonical repair, `POST /api/datasets/:id/repair`

**Status: design note only. Nothing in this note is implemented; implementation needs explicit approval (P2 brief).** The programme constraints apply throughout: the on-disk store format and RocksDB layout stay unchanged (the chase runs in a throwaway in-memory oxigraph store; a persisted proposal is a file under `--data-dir`, section 7.3); every existing route, status code and JSON field keeps its meaning (everything here is an addition, and the two parameters added to the patch route are opt-in); `src/auth/`, the frontend, CI and the dependency sections of `Cargo.toml` are not touched. Every claim about current behaviour cites `path:line` in the `feat/improvements` worktree; what the tree cannot establish is marked *unmeasured* or *not established*, with the measurement that would settle it.

**Ordering, per the maintainer's steer.** The reliable changelog — per-quad change capture with a durable cursor — is the first deliverable of P2 and the prerequisite for this layer's base marker and its apply precondition (section 1.3). SHACL→SQL is *deferred pending measurement*, not rejected; the gating experiment is specified in section 5.5 and bulk revalidation after a shape or model update is treated as a real, recurring workload (section 5.4).

## 1. Purpose and position in the pipeline

### 1.1 What the layer is

A deterministic step that **proposes** a change to a dataset and never applies it. Input: a dataset, a rule set (authored, or compiled from the dataset's SHACL Core shapes and OWL axioms), a budget. Output: an RDF Patch plus a structured report, derived by running a restricted chase to a budgeted fixpoint over an in-memory copy of the dataset. A human, or a later policy, applies the patch through the existing patch route, which this design extends with the gate re-validation it lacks today.

### 1.2 Relation to what exists

| Existing mechanism | Relation |
|---|---|
| SHACL write gate (`check_write_gates`, src/shacl_studio/gate.rs:111) | Rejects a non-conforming payload with 422 (src/server/error.rs:57-73); never proposes a fix. Its temp-store pattern (gate.rs:138-147) is the sandbox precedent (section 5). |
| SHACL-AF rules (`shacl::infer`) | Materialise in place, one `store.update` per focus node (src/shacl/engine.rs:1478), converging on a `store.len()` delta under a hard cap of 100 rounds (engine.rs:230-231, :280-288). They cannot express equality, and an existential head never converges under that loop (section 2). SHACL-AF rules are *importable* as TGDs; they are not executed through `infer`. |
| OWL 2 RL (`Owl2RLReasoner`) | Derives `owl:sameAs` from prp-fp/prp-ifp/prp-key/cls-maxc2 (src/reasoning/owl2_rl.rs:532-547, :661-680, :1049-1050) but never merges a node; eq-rep-* only copy (owl2_rl.rs:456-458). It "cannot generate" existential witnesses (docs/owl2-rl.md:177). Key and functional axioms become EGDs here; existentials become TGDs with labelled nulls. |
| LLM features | No LLM receives a validation report today: `ShaclAssistRequest` carries `task/description/turtle/model_context/model` only (src/server/llm_sparql.rs:470-488); Spark's `validate_sparql` is `parse_query` (llm_sparql.rs:1030-1034) and its prompt refuses data modification (llm_sparql.rs:1239); the saved-query "repair" returns a suggestion that is only persisted with `save: true` (src/saved_queries/handlers.rs:529-541, :659-660). The repair layer is the deterministic step *before* any LLM correction (section 9). |

The correction loop today ends at a human reading JSON from `POST /api/datasets/:id/validate` (src/server/routes.rs:7265) or a report graph appended per pipeline run (src/shacl_studio/exec.rs:279-298). After this design: validate → chase-propose → review → gated apply → validate.

### 1.3 Prerequisite: the changelog first

Two facts make the changelog the prerequisite rather than a parallel item. The store's write generation is a process-local `AtomicU64::new(0)` (src/store/query_cache.rs:127) and commit ids are uuid-v4 (src/commit_log.rs:126), so nothing durable says "the dataset has not changed since this proposal was computed"; and per-triple provenance is explicitly absent pending per-write change capture (src/provenance.rs:12-14). The change-capture follow-up establishes that five engine primitives already hold the exact delta and discard it (e.g. `update_targeted_delta` at src/store/engine.rs:969-970), and that the narrowest correct hook is inside each primitive, not at `begin_write` (which carries no payload, engine.rs:430-434) nor at the HTTP layer (which never sees seed loads, recovery, reasoners or plugins). This note therefore assumes a **change record per write with a monotonically increasing sequence** exists before the endpoint ships, and uses it for the base marker (section 7.1) and the apply precondition (section 8.3); until then those fields carry the commit id and the process-local generation as best-effort stand-ins.

One requirement on CC follows from the commit log's own ordering: today the commit record is written *after* the data write (the patch route runs `st.store.update` at src/rdf_patch.rs:598 and `commit_log::record` at :614; the record itself is a second `store.update`, commit_log.rs:261-277), so a proposal computed between the two carries the previous commit id and a commit-based precondition passes although the base has moved. The change-capture sequence must therefore be assigned inside the write's own primitive (the hook location the follow-up identifies), not by a later HTTP-layer call, or the window simply moves.

## 2. What exists today

**Rule execution loops and termination.** Three loops, all "run every rule, stop when a whole round adds nothing": SHACL-AF, `for iteration in 0..100` with `store.len()` before and after (engine.rs:230-231, :280); rules are `Rule { shape_iri, targets, rule_type, body, order: f64, conditions }` (engine.rs:1285-1292) loaded by the private `load_rules` (engine.rs:1326), `apply_rule` substitutes `$this` textually (engine.rs:1444) and splices `WITH <g>` after the prologue (engine.rs:1457-1463); a `sh:TripleRule` becomes `INSERT DATA` with no `WHERE` (engine.rs:1470-1471). OWL 2 RL, `MAX_ITERATIONS = 500` (owl2_rl.rs:82), `if added == 0 || iterations >= MAX_ITERATIONS { break; }` (owl2_rl.rs:388) with `count_graph` per round (owl2_rl.rs:385; src/reasoning/common.rs:58). SWRL, default 100 rounds capped at 1000 (src/server/routes.rs:8924, :8959).

**Existential heads do not converge.** spareval clears the template blank-node map after every solution (`self.bnodes.clear()`, spareval-0.2.6/src/update.rs:65-69), `convert_blank_node` mints `BlankNode::default()` per label (update.rs:196-198), a random id (oxrdf-0.3.3/src/blank_node.rs:108). A `sh:construct` with a blank node in its template inserts a fresh witness per focus node per round and runs all 100 iterations. No test covers it: `tests/shacl_rules_conformance.rs` contains no `_:`, and the only idempotence test uses a ground head (tests/shacl_rules_conformance.rs:242).

**Equality without merging.** `IdentityPolicy::{Off, Narrow (default), Full}` (src/reasoning/identity.rs:30-45); eq-* rules run only when `propagates_same_as()` (owl2_rl.rs:377-383), so under `sameas-off` derived sameAs is inert. The raw engine defaults to `Full` (owl2_rl.rs:280), per-dataset runs to `Narrow` (src/entailment.rs:454-472). `has_keys()` walks `owl:hasKey` lists with a 64-cell cap (owl2_rl.rs:687, :729).

**The gate's sandbox.** `check_write_gates` builds `TripleStore::in_memory()` (gate.rs:138), seeds the target graph by Turtle dump and re-parse on `WriteMode::Merge` (`seed_existing_graph`, gate.rs:55-67), copies shape graphs the same way (`copy_shape_graphs`, gate.rs:367), and validates `data_graphs = [graph_iri]` only with `run_inference=false` ("gating must not mutate any store", gate.rs:392-395, :401-407, :419-425). `check_import_gates` re-homes parsed quads with `bulk_insert_quads` instead (gate.rs:198-212). The comment there claiming the round trip "would also relabel blank nodes" (gate.rs:198-199) is wrong for the current configuration: `parser_for` never enables renaming (src/store/engine.rs:1359-1367), oxrdfio defaults `rename_blank_nodes: false` (oxrdfio-0.2.5/src/parser.rs:109), and the temp store is built with `BlankNodeMode::default()` = Preserve (engine.rs:371, :64-83). `run_validation` already takes `run_inference` and calls `crate::shacl::infer` first (src/shacl_studio/run.rs:40-45); `run_validation_capturing` recovers which quads inference added (run.rs:70-71).

**RDF Patch.** `parse` (src/rdf_patch.rs:279) skips `#` lines (:286-288), stores headers as untyped strings (:299-308) and reads only `id` (:62-67); blank nodes are refused in `D` lines (`!delete`, :354-358) because apply is `DELETE DATA` (:1-15); triple terms cannot be tokenised (:103-111). `to_sparql_update` emits one block per polarity run joined by `;` (:387, :427-438). `generate` diffs two graphs of the **same** store by `"{s} {p} {o}"` string sets (`triples_of` :442; :460-461; sorted D then A per graph, :485-500); its one caller is the version diff (src/dataset_versions/handlers.rs:736). `apply_patch_handler` (:507) returns 404 for missing or inaccessible (:519, :525), 403 for not writable (:532), 415 on Content-Type (:539-543), 400 for an unregistered graph (:582-586), runs `ldes::capture::before` → one `st.store.update` → `capture::after` → `entailment::after_write` in `spawn_blocking` (:596-602), syncs the text index (:611-613) and records `CommitKind::Sparql` with A/D line counts (:614-629). It calls no SHACL gate: the gate call sites are exactly routes.rs:1445, :1636, :1720, src/dataset_versions/commit.rs:287 and src/imports/handlers.rs:482. It is mounted in `dataset_mixed_routes` under `optional_auth` (src/server/mod.rs:1161-1164, :1201) with a required `Extension<AuthenticatedUser>` (:509), so an anonymous POST fails with 500 "Missing request extension" (axum-0.7.9/src/extract/rejection.rs:42-47) — by construction of `optional_auth`; no test exercises it.

**Proposal-before-apply patterns.** Saved-query repair with `save=false` (handlers.rs:529-541); model `GET /api/models/:id/merge/preview` → `POST …/merge` (src/data_models/merge.rs:146; src/data_models/routes.rs:35, :66); RML `?preview=true` (routes.rs:7971). `execute_pipeline_dry` (src/shacl_studio/exec.rs:424-426) still materialises inference in place, so it is not a sandbox. No proposal entity exists.

**Blank-node mode.** `BlankNodeMode { Preserve (default), Canonical, Skolem }` (engine.rs:64-83); `apply_blank_node_mode` has one call site, in `parse_quads` (engine.rs:1260), never for updates or `bulk_insert_quads`; no production caller of `with_blank_node_mode`. `opengraph::skolem` provides `DEFAULT_SKOLEM_BASE = "https://opengraph.local"`, `GENID_PATH = "/.well-known/genid/"`, `skolemize`, `is_skolem_iri`, `deskolemize` (opengraph/src/skolem.rs:26, :29, :46, :76, :84).

**Validation reports** carry seven fields, no graph (src/shacl/report.rs:27-35); `source_constraint` is free-form (`format!("sh:minCount {}", min)`, src/shacl/constraints.rs:234); `value` is `lit.value()` with datatype dropped (:47). `load_shapes` is private (engine.rs:299) while `Shape`/`Target`/`PropertyShape`/`PropertyPath`/`Constraint` are `pub` (src/shacl/shapes.rs:9, :32, :44, :63, :110; 31 `Constraint` variants).

## 3. Rule format

### 3.1 Decision: a small vocabulary, SHACL-AF compatible, executed by its own evaluator

SHACL-AF alone was rejected as the carrier for three reasons: it has no equality-generating head; an existential head is a template blank node the engine mints fresh per solution (section 2); its executor is one `store.update` per focus node through `WITH <g>` (engine.rs:1457-1463), which makes `<g>` the default graph of the `WHERE` so a body cannot join against model or entailment graphs, and gives no per-derivation provenance. But its *syntax* is right: a rule is a `CONSTRUCT` template plus a `WHERE` pattern, and `sh:order`/`sh:condition`/`sh:deactivated` map onto priority, guard and switch. So:

**A rule is a SPARQL `CONSTRUCT … WHERE …` string** (the `chase:construct` property; a `sh:SPARQLRule`'s `sh:construct` is accepted verbatim), parsed with spargebra's `parse_query` — the same parser `validate_sparql` uses (llm_sparql.rs:1030-1034). The head grammar is therefore exactly SPARQL's `ConstructTemplate`; the body grammar is `GroupGraphPattern`, restricted by a walk over the parsed algebra to BGP, `FILTER`, `BIND`, `VALUES`, `FILTER NOT EXISTS`/`MINUS` and `GRAPH ?g { … }` (no `OPTIONAL`, no top-level `UNION`, no `SERVICE`, no aggregates). Functions registered by `query_options()` (src/store/engine.rs:558-600: GeoSPARQL at :573-576, ADJUST at :591-595, `sh:SPARQLFunction`s discovered in the store at :597-600) are available. Additional terms in the platform namespace `https://opentriplestore.org/ns#` (already used for graph roles, src/auth/dataset_graph.rs:548; written `ots:` below):

| Term | Value | Meaning |
|---|---|---|
| `ots:nulls` | RDF list of head variables not bound by the body | labelled nulls (existential TGD) |
| `ots:equate` | list of exactly two body variables | EGD head; the template must then be empty |
| `ots:retract` | a second `ConstructTemplate` string | triples deleted when the rule fires (replacement rules: datatype relabel, unit normalisation, `sh:closed`) |
| `ots:mergeMode` | `ots:SameAsOnly` (default) / `ots:Rewrite` | how a live/live merge is proposed (section 4.4) |
| `ots:graphScope` | `ots:PerGraph` (default for compiled rules) / `ots:Union` | body atoms must share one graph, or match across the premise union |
| `ots:targetGraph` | IRI, or `ots:FocusGraph` (default) | where head triples land (section 4.6) |
| `ots:priority` | decimal, default 0 (same convention as `sh:order`) | order within a stratum |
| `ots:policy` | `ots:Repair` (default) / `ots:Report` | `Report` rules fire and are counted but emit no lines |
| `ots:confidence` | `certain` / `policy` / `heuristic` | copied into the report |
| `ots:derivedFrom`, `ots:destructive`, `ots:message`, `sh:deactivated` | — | provenance, flag, explanation template, switch |

The checker rejects a template blank node (use `ots:nulls`), a head variable that is neither body-bound nor a declared null, and a `MINUS`/`NOT EXISTS` over a predicate produced by the same or a higher stratum (section 3.4). A `sh:condition` is honoured by evaluating the inline shape against each candidate focus node in the sandbox with the engine's own `validate_inline_shape` (the mechanism `sh:node` uses, constraints.rs:931), not by rewriting the shape to SPARQL — no shape-to-SPARQL renderer exists; `prebind` (constraints.rs:1105) only injects `VALUES`/`FROM` into a user-written query (constraints.rs:1305).

### 3.2 Worked example 1: key-based merge EGD from `owl:hasKey`

Compiled from `ex:Asset owl:hasKey ( ex:code )` found in a model graph, with the same list walk as `has_keys()` (owl2_rl.rs:687-729) and the same `isIRI` filters as prp-key (owl2_rl.rs:673):

```turtle
<urn:ots:rule:haskey:ex-Asset:1> a ots:Rule ;
  ots:derivedFrom ex:Asset ; ots:confidence "certain" ;
  ots:construct """CONSTRUCT { } WHERE {
    ?x a ex:Asset ; ex:code ?k .  ?y a ex:Asset ; ex:code ?k .
    FILTER(?x != ?y) FILTER(isIRI(?x)) FILTER(isIRI(?y)) }""" ;
  ots:equate ( ?x ?y ) ; ots:mergeMode ots:SameAsOnly ;
  ots:message "{?x} and {?y} share key ex:code = {?k}" .
```

Blank-node subjects are excluded because a merge involving one cannot be expressed in RDF Patch (:354-358). With `SameAsOnly` the proposal contains one line, `A <loser> owl:sameAs <winner> <g> .`; with `Rewrite`, `D`/`A` pairs for every triple of the loser.

### 3.3 Worked example 2: mandatory-part completion TGD from `sh:minCount`

From `ex:BridgeShape sh:property [ sh:path ex:hasDeck ; sh:minCount 1 ; sh:class ex:Deck ]`:

```turtle
<urn:ots:rule:mincount:ex-BridgeShape:hasDeck> a ots:Rule ;
  ots:derivedFrom ex:BridgeShape ; ots:graphScope ots:PerGraph ;
  ots:construct """CONSTRUCT { ?this ex:hasDeck ?w . ?w a ex:Deck . } WHERE {
    GRAPH ?g { ?this a ex:Bridge . }
    FILTER NOT EXISTS { GRAPH ?g { ?this ex:hasDeck ?w0 . } ?w0 a ex:Deck . } }""" ;
  ots:nulls ( ?w ) ;
  ots:message "{?this} needs at least one ex:hasDeck of class ex:Deck" .
```

The `FILTER NOT EXISTS` is the restricted-chase guard (section 4.2), generated by the compiler: the full head with each null replaced by a fresh variable. The *value* atom (`?this ex:hasDeck ?w0`) is inside the same `GRAPH ?g`, reproducing the engine's per-graph hop confinement for IRI focus nodes (constraints.rs:1533-1540); the *typing* atom (`?w0 a ex:Deck`) is deliberately left **outside** `GRAPH ?g`, because the engine's `sh:class` check (`is_instance_of`) reads with `GraphSel::All` (src/shacl/view.rs:715-724, followup-shacl-internals Q5) — a deck typed in another graph satisfies the validator, so it must also satisfy the guard, or the chase would mint a witness the validator never asked for (section 4.1). A union-scoped *value* atom would have the opposite defect: a deck linked in another graph would silence the guard while the validator still reported the violation.

### 3.4 Worked example 3: parthood closure TGD (ground, recursive)

```turtle
<urn:ots:rule:parthood-closure> a ots:Rule ; ots:graphScope ots:Union ;
  ots:construct """CONSTRUCT { ?x nen2660:hasPart ?z } WHERE {
    ?x nen2660:hasPart ?y . ?y nen2660:hasPart ?z . FILTER(?x != ?z) }""" .
```

No nulls, so termination follows from finiteness of the active domain; the guard is injected anyway so the per-round delta is exact. This duplicates what `materialize` mode derives into `urn:entailment:{regime}:{dataset}` (src/entailment.rs:46-48) when the property is declared transitive, so the compiler never generates closure rules from `owl:TransitiveProperty`; users author them when they want the closure in the data graphs. The natural companion is `nen2660-relations`, whose acyclicity checks are `sh:sparql` (examples/seed-bundles/nen2660-relations/shapes.ttl:26).

**Unit normalisation** (the brief's fourth use case) is a replacement rule: body `?x ex:length ?v ; ex:unit "mm" . BIND(?v / 1000 AS ?m)`, head `?x ex:length ?m ; ex:unit "m"`, `ots:retract` `?x ex:length ?v ; ex:unit "mm"`, `ots:confidence "policy"`, `ots:destructive true`. A replacement rule is an ordinary rule: its head is added and its retract applied **in the same pass** (the 4.7 pseudo-code), and it sits in whatever stratum the dependency analysis assigns it, so rules that read the normalised value (`ex:unit "m"`) see it. It is self-guarded (the retract atoms are absent after firing), hence idempotent. Only *policy* deletions (`closed-delete`, `maxCount-keep-lexmin`, the loser's triples under `Rewrite`) are confined to the last stratum (section 6).

**Stratification** is computed, not declared. Two kinds of negative edge: (a) a predicate inside a `NOT EXISTS`/`MINUS` of rule R depends negatively on every rule whose head or retract mentions it; (b) a retract is non-monotone, so every rule whose *body* positively reads a retracted predicate p depends negatively on the retracting rule, and the retracting rule depends negatively on every rule whose *head* produces p (producers of p < retractor of p < consumers of p). A variable in predicate position — in a head, a retract, or a negated pattern — is treated as mentioning **every** predicate, so `NOT EXISTS { ?s ?p ?o }` and the compiled `sh:closed` retract `focus ?p ?o` cannot escape the check. The dependency graph must have no cycle through a negative edge; a cycle is a load-time 400 naming the rules (for instance a closure rule that both produces and consumes `hasPart` together with a rule retracting `hasPart`). The head guard is exempt (a self-loop on one rule is the definition of a restricted step). `ots:priority` orders within a stratum only.

### 3.5 Declaring rules per dataset

No new `GraphKind` (`GraphKind` is a content classification, src/auth/models.rs:576-577, :610-622; adding a variant is a `src/auth` edit). Rules live (a) in a shapes graph (`GraphKind::Shapes`, already excluded from reasoning premises by `is_reasoning_source`, src/conformance.rs:70-81), resolved by `dataset_shapes_sources` exactly as `/validate` and `/infer` resolve shapes (routes.rs:7186, :7768-7770); (b) in rule graphs named in the request; or (c) compiled per run from the effective shapes and the OWL axioms in `entailment::reasoning_sources` (entailment.rs:479-494). The compiled set is returned in the report so it can be saved and edited.

### 3.6 Derivation from SHACL Core shapes

**Corpus** (followup-shacl-internals Q1). Ten counted non-W3C shape graphs — nine vendored plus the git-ignored `examples/seed-bundles/nen2660-imbor/nen2660-shacl.ttl`, present locally after a `fetch.sh` run — total minCount 44, maxCount 36, datatype 36, class 63 (58 of them in nen2660-shacl.ttl), in 12 (6 in nen2660-shacl.ttl), pattern 6, sparql 18, nodeKind 31; zero custom components, zero inverse/oneOrMore/zeroOrOne paths. nen2660-shacl.ttl alone is class 58, datatype 8, in 6, or 5, qualifiedValueShape 4, maxCount 3, minCount 2, with four `sh:path (rdf:type [sh:zeroOrMorePath rdfs:subClassOf])` sequence paths. Summing the follow-up's per-file lines for the **nine vendored** graphs gives minCount 40, maxCount 32, datatype 27, nodeKind 22, sparql 18, in 6, pattern 6, class 5 (the follow-up's ten-file totals do not reconcile exactly with its per-file lines; the per-file sums are used here). The IMBOR Kern shape graph — declared as a model graph at examples/seed-bundles/nen2660-imbor/manifest.toml:60-66 and bound as the sample dataset's only `shape_graphs` entry at manifest.toml:93 — is fetched, not vendored (fetch.sh:17), so its mix is *not established*. Repairability follows the evaluation arm in src/shacl/constraints.rs; only `PropertyPath::Predicate` and `Inverse(Predicate)` are invertible.

| Constraint | Rule | Policy | Why |
|---|---|---|---|
| `sh:hasValue v` (:352) | TGD add `focus path v` | on, certain | value is in the shape |
| `sh:class C` (:168) | TGD add `v rdf:type C` | on, certain | offending value bound; IRI values only |
| `sh:minCount n` whose sibling constraints all admit an IRI witness (`sh:class`, `sh:nodeKind sh:IRI`/`sh:BlankNodeOrIRI`, `sh:node`, or no sibling) | existential TGD, one null per missing count | on, certain | no other triple satisfies it, and the witness violates no sibling |
| `sh:minCount n` with single-member `sh:in` / `sh:hasValue` | ground TGD | on, certain | value determined |
| `sh:minCount n` with a sibling a Skolem IRI cannot satisfy — `sh:datatype`, `sh:nodeKind` `Literal`/`BlankNode`/`BlankNodeOrLiteral`/`IRIOrLiteral`, `sh:pattern`, `sh:in` (>1), length/range, `sh:languageIn` | none | Report | a null cannot be a literal and must not create a new violation on the shape it repairs |
| `sh:datatype dt` (:186) | replacement `"lex"^^old → "lex"^^dt` iff `xsd_lexical_valid` (:1940) for `dt` | opt-in `datatype-relabel`, policy | relabel only; ill-formed forms are residue; the sandbox is re-read because the report drops the datatype (:47) |
| `sh:closed` (:425-427) | retract `focus p o` | opt-in `closed-delete`, policy, destructive | exact triple identified |
| `sh:maxCount` (:241-243), `sh:uniqueLang`, qualified max | none by default | opt-in `maxCount-keep-lexmin` keeps the N lexicographically smallest N-Triples values | the engine records no ordering of surplus values |
| `sh:in` (>1), `sh:pattern`, length/range, `sh:languageIn`, `sh:nodeKind` | none | Report | shape constrains but does not determine the value |
| `sh:equals`, `sh:disjoint`, `sh:lessThan*` | none | Report | authoritative side undetermined |
| `sh:node`, `sh:and` | recurse over the shape model | as inner | inner results are discarded by the engine (:931) |
| `sh:or`, `sh:xone`, `sh:not`, `sh:qualifiedValueShape` | none | Report | needs a disjunct choice or an anti-repair |
| `sh:sparql`, custom, `sh:expression` (:437, :512) | none | out of reach | no head to invert |

A minted witness carries a typing atom (`?w a C`); on a `sh:closed` shape whose `sh:ignoredProperties` omits `rdf:type` the validator will flag it (`sh:closed` scans every predicate of the focus node, constraints.rs:425-427). The compiler therefore compiles such a `minCount` to Report when the *witness's* class shape is closed without `rdf:type` ignored, and `closed-delete` never retracts a derived quad (section 6) — combining the two on the same shape is reported as a rule conflict, not resolved.

**Expected share on the vendored corpus.** Of the nine vendored graphs, the "on, certain" rows cover at most the 5 `sh:class` and up to 40 `sh:minCount` occurrences; how many of those 40 carry a `sh:datatype`/`sh:nodeKind`-literal sibling (and so fall to Report) is *not established* — a grep census counts components, not sibling pairs; the R0 `load_shapes` visibility change makes that pairing computable and it should be recorded then. The remainder is Report or opt-in by construction: maxCount 32 (opt-in policy), datatype 27 (opt-in relabel), nodeKind 22, sparql 18 (out of reach), in 6, pattern 6. So on today's vendored shapes the certain, on-by-default repair covers **at most ~45 of ~156** constraint occurrences, and the real target (IMBOR Kern, class-heavy per its manifest description) is unmeasured; the run-time `report_only`/`unexpressible` counts of section 7.2 exist precisely because this share must be observed, not assumed.

Targets: `sh:targetClass` (confined per graph, engine.rs:1196-1217) compiles to `GRAPH ?g { ?this a C }` with the `rdfs:subClassOf*` closure expanded at compile time **per graph, with the same walk the engine uses** (engine.rs:1196-1217), not over the model graphs — widening the closure to the model graphs is open question 2, and until it is answered the compiler and the validator must agree or the before/after counts diverge; `sh:targetSubjectsOf`/`ObjectsOf` (`GraphSel::All`, engine.rs:1223-1226) compile with `Union` scope; `sh:targetNode` to `VALUES`; SPARQL targets are not compiled.

**The implicit-class-target gap.** The engine's implicit target fires only when `<shape> a rdfs:Class` is in the *shapes graph itself* (`ASK { GRAPH <shapes_graph> … }`, engine.rs:455-460), while `nen2660-imbor`'s manifest puts the `rdfs:Class` assertions in a different graph, so 27 of the 30 NEN 2660 node shapes get no targets (followup-shacl-internals Q1 entry J; rests on the git-ignored file, not reproducible from a clean checkout). The compiler resolves implicit targets over the sandbox's model graphs as well as the shapes graph, so those shapes compile; but the before/after validation counts (section 7.2) still come from the engine and will under-report for them. Whether the engine's ASK should also consult the model graphs is an in-scope `src/shacl/` change that alters validation results and is left as an open question.

### 3.7 Derivation from OWL axioms

From `entailment::reasoning_sources` (model and vocabulary graphs plus the conformed model version's graphs, src/conformance.rs:107-108): `owl:hasKey` → EGD (3.2); `owl:FunctionalProperty` → EGD on objects (prp-fp, owl2_rl.rs:532-535); `owl:InverseFunctionalProperty` → EGD on subjects (:547); `owl:someValuesFrom` / `owl:minCardinality ≥ 1` → existential TGD. `owl:maxCardinality 1` (cls-maxc2, :1049-1050) is *not* compiled by default: a restriction on a class-expression node is too easily satisfied by unrelated blank nodes. `rdfs:domain`/`range`/`subClassOf` are not compiled by default because a dataset in materialize mode with an rdfs-or-above regime already derives them into `urn:entailment:{regime}:{id}` after every write (docs/reasoning.md:50-56); a request flag enables them for datasets in `query` mode or below rdfs. Existing `owl:sameAs` seeds the equality structure only when the dataset's effective identity policy propagates it (`effective_identity`, entailment.rs:454-472; `propagates_same_as`, identity.rs:70), and Linkset graphs are premises only under `sameas-full` (entailment.rs:482-491): the chase inherits the per-dataset policy, not the raw engine's `Full` default nor the unscoped `POST /api/reasoning/materialize` default (routes.rs:8655).

## 4. Chase semantics

### 4.1 Restricted, not oblivious

A trigger (rule R, body match σ) is *active* only if its head is not already satisfied by some extension of σ. The oblivious chase — every trigger once — is what `infer` does today and what mints 100 witnesses per focus node. The restricted chase makes a proposal minimal: no witness when a satisfying one exists.

### 4.2 Idempotent head-satisfaction check

For a TGD with head H the compiler appends `FILTER NOT EXISTS { H[nulls ↦ fresh variables] }` — the complete head, including the typing atom on the witness (a projection would be an oblivious step on a partial head). Under `PerGraph` scope the value atoms sit inside the same `GRAPH ?g`; typing atoms sit outside it, matching `is_instance_of`'s `GraphSel::All` (3.3). This is the phrasing verified against the engine in followup-shacl-internals Q4: SPARQL variables, never template labels. Because heads are applied in Rust (4.7) with a `contains` check, a second run over the applied proposal is a no-op and the count of proposed additions is exact.

### 4.3 Labelled nulls are Skolem IRIs

Nulls are IRIs, not blank nodes: (a) a `D` line with a blank node is refused (rdf_patch.rs:354-358), so a null that a later EGD merges away, or that a reviewer retracts with an inverse patch, must be nameable; (b) an `A` line with a blank node is applied as `INSERT DATA`, which creates a fresh blank-node map per execution (oxigraph-0.5.9/src/sparql/update.rs:296), so a later patch can never refer to the node; (c) production is always `Preserve` (engine.rs:371). The IRI is

```
{base}/.well-known/genid/{sha256(rule_iri ‖ sorted frontier bindings in N-Triples ‖ null_name)[..32]}
```

reusing `GENID_PATH` (skolem.rs:29). Content-derivation gives determinism: the same state and rules mint the same IRIs, so re-running after a partial apply is idempotent by set semantics. `is_skolem_iri` (skolem.rs:76) identifies nulls in later runs so an EGD can unify a null with a live term. `skolemize` itself (skolem.rs:46) hashes structure after the fact and cannot be used at evaluation time. Collisions with user IRIs are not analysed (a 128-bit prefix under a reserved path; no measurement) but are checked: a minted IRI already present in the sandbox with a different `ots:derivedFrom` makes the firing residue. The base (`DEFAULT_SKOLEM_BASE`, skolem.rs:26, or `state.base_url`) is an open question; serving `/.well-known/genid/` is not required (the dev proxy forwards `/.well-known`, frontend/vite.config.js:149, but no route exists).

### 4.4 EGD application and the canonical representative

An EGD trigger yields terms `a`, `b`; the evaluator keeps a union-find over terms.

1. If either is a literal, or the pair is asserted distinct — `owl:differentFrom` between them or membership of one `owl:AllDifferent` list (individual level; the role eq-diff1 plays as an ASK, owl2_rl.rs:491-497), or their asserted classes are `owl:disjointWith` or members of one `owl:AllDisjointClasses` list in the premises (class level; `nen2660:AllDisjointClassesShape` is a `sh:sparql` check of exactly this, followup-shacl-internals Q1 entry J) — **conflict**, recorded, no merge.
2. Otherwise the representative is chosen by a **total order that depends only on the input**: a live IRI beats a null; among live IRIs the lexicographically smallest N-Triples form wins. Rationale: it is independent of evaluation order and of which rule fired first — the property that makes "canonical" meaningful (section 6) — it never lets a placeholder win over an asserted identifier, and it is the byte order `generate` already sorts patch lines by (rdf_patch.rs:489-490). "Most connected" was rejected because degrees change during the chase, making the choice order-dependent; "oldest" is unavailable because the commit log records graphs, not triples (provenance.rs:12-14).
3. Merging a null into a term is a **substitution**: every sandbox quad mentioning the null is rewritten (recorded as delete + add with `substitution` provenance). Merging two live IRIs follows `ots:mergeMode`: `SameAsOnly` adds `loser owl:sameAs winner` (one line, reversible, consistent with what RL derives and eq-rep-* copy into `urn:entailment:…` on the next materialisation); `Rewrite` emits `D` for every triple of the loser and `A` for its image, marked destructive. Merged pairs re-feed the TGD strata until no new class forms.

Downstream effects of `Rewrite` that a reviewer must know: deleting every triple of the loser makes `ldes::capture::after` write a tombstone for it (src/ldes/capture.rs:145-161) and drops its text-index document; `SameAsOnly` avoids both. Under `sameas-off` the added link is inert, as today (owl2_rl.rs:372-383).

### 4.5 Termination budget

| Budget | Default | Cap | Source of default |
|---|---|---|---|
| rounds per stratum | 100 | 1000 | SHACL-AF's cap (engine.rs:230) and SWRL's (routes.rs:8959); a guarded rule is expected to converge in 2 rounds, but no test exercises one (R0 adds it) |
| nulls minted | 10 000 | 1 000 000 | unmeasured; bounds a runaway existential rule |
| proposed ops | 50 000 | 500 000 | at an assumed ~100 B/line (unmeasured) the default keeps the patch under the 8 MiB global body limit (mod.rs:2010); a proposal near the cap (~50 MB) exceeds it and can only be applied through the server-side `proposals/:pid/apply` resource (8.2), never by re-POSTing it to `/patch` |
| wall clock | `state.query_timeout_secs` (30, mod.rs:336) | 240 s | the query bound (routes.rs:642, :710-713), inside the 300 s global `TimeoutLayer` (mod.rs:2004-2007); the default is known to be too small for 9M-quad datasets if the seed extrapolation of 5.4 holds, and is re-set from the 5.5 measurement |
| sandbox quads | section 5.4 | | |

Exhausting a budget is not an error: the proposal is returned with `exhausted: {kind}` and is a valid partial repair, because every emitted op came from an active trigger against the state at that point. Convergence is measured on the sandbox's memory store, where `len()` is cheap — an argument for the sandbox over an in-place chase on RocksDB, where `store.len()` is O(N).

### 4.6 Head placement in multi-graph datasets

The report has no graph field (report.rs:27-35), so the chase never uses the report. Each body atom is rewritten as `GRAPH ?_gi { atom }` and the evaluator records the graph per matched atom. A head triple lands in `ots:targetGraph` when set; else in the graph that bound the head subject's typing atom; else the graph of the first body atom binding the subject. The target must be a dataset-registered graph that is not `Entailment`/`System`/`Shapes`/`Provenance` and not a model graph from `reasoning_sources`; otherwise the trigger is `unplaceable`. Per-graph confinement is incidental, not contractual (no test pins it, followup-shacl-internals Q5); a two-graph fixture must assert it before R1 relies on it.

### 4.7 Evaluator: data structures and algorithm

The evaluator does not write through SPARQL UPDATE. Bodies run as `SELECT` over the sandbox (through `query_options()`, so custom functions are available); heads are applied in Rust.

```rust
struct Derivation { rule: RuleId, round: u32, trigger: [u8; 32] /* hash of sorted bindings */,
                    premises: Vec<Quad>, kind: Added | Deleted | Substituted { from: Term, to: Term } }
struct ChaseState {
    derived: Vec<(Quad, Derivation)>, derived_set: HashSet<Quad>,   // insertion order = determinism
    deleted: Vec<(Quad, Derivation)>,
    nulls:   HashMap<(RuleId, [u8;32], String), NamedNode>,
    uf:      UnionFind<Term>,                                        // ~40 lines, no crate
    merges:  Vec<Merge>, conflicts: Vec<Conflict>, unplaceable: Vec<Trigger>, unexpressible: Vec<Quad>,
    budget:  BudgetUsed,
}
```

```
chase(sandbox, rules, budget):
  strata = stratify(rules)?                                  // Err → 400; policy deletions form the last stratum
  for stratum in strata:
    for round in 1..=budget.rounds:
      new = []; gone = []
      for rule in stratum sorted by (priority, rule_iri):
        sols = eval(body_query(rule, round)); sols.sort_by(ntriples_tuple)
        for σ in sols:
          match rule:
            Tgd => for n in nulls: σ[n] = mint(rule, hash(σ|frontier), n)
                   for q in instantiate(head, σ, target_graph(rule, σ))?:   // None → unplaceable
                     if bnode_or_triple_term(q) → unexpressible
                     else if !sandbox.contains(q) && !derived_set.contains(q): derived.push(q); new.push(q)
                   for q in instantiate(retract, σ):                        // replacement rules, same pass
                     if sandbox.contains(q) && !derived_set.contains(q): deleted.push(q); gone.push(q)
                     else if derived_set.contains(q): conflicts.push(..)
            Egd(a,b) => (ra, rb) = (uf.find(σ[a]), uf.find(σ[b])); if ra == rb: continue
                   if incompatible(ra, rb): conflicts.push(..); continue
                   (win, lose) = representative(ra, rb); uf.union(lose, win); merges.push(..)
                   if is_null(lose): substitute(lose → win) everywhere
                   else if SameAsOnly: new.push(lose owl:sameAs win @ graph_of(lose))
                   else: for q in sandbox.quads_mentioning(lose): gone.push(q); new.push(q[lose↦win])
          budget.check()?                                     // → exhausted, break cleanly
      if new.is_empty() && gone.is_empty(): break
      sandbox.apply(new, gone); sandbox.set_delta(new)
```

Semi-naive evaluation for rounds > 1 rewrites a body with atoms A₁…Aₙ into the union over i of "Aᵢ from `urn:ots:chase:delta`, the others from the full sandbox"; guards always read the full sandbox. Phase R1 may ship naive re-evaluation (correct given guard plus `contains`) and add semi-naive later. Determinism: total rule order, sorted solutions, content-derived nulls, total representative order, insertion-ordered derivations; two runs over the same snapshot and rules produce byte-identical **patch text** — which is why every patch header is itself content-derived or snapshot-derived (7.1) and the volatile fields (`elapsed_ms`, `write_generation`) live only in the JSON report.

## 5. Sandbox

### 5.1 Decision: temp in-memory store, seeded from a graph set

*Staging named graph in the live store* (the `urn:ots:restore:{uuid}` + `MOVE` pattern, src/dataset_versions/snapshot.rs:159-180): every chase write goes through `begin_write` (mirror `write_started` + query-cache invalidation, engine.rs:430-434, again on guard drop, :306-311), a recount or full graph-index rebuild per update (engine.rs:769-774; MOVE desugars to two `Drop`s plus a copy, spargebra-0.4.6/src/parser.rs:1294-1301, and `Drop` hits the `_ => return None` arm at engine.rs:832, forcing a rebuild), no LDES/entailment fan-out because those are HTTP-layer only, a staging graph visible to admins over `/store`, and a proposal that mutates the source of truth against the gate's own rule (gate.rs:394-395). Rejected.

*Temp in-memory store*, as the gate does (gate.rs:138): writes are cheap, `len()` is cheap, nothing is observable outside the request, dropping it is the rollback. Chosen, with two differences from the gate: seeded from a **set** of graphs, and by copying quads directly into `bulk_insert_quads` (engine.rs:1624-1626 — the path `check_import_gates` uses, gate.rs:211), not by Turtle dump and re-parse. Labels are then identical by construction and no serialisation cost is paid. The read side is *not* today's `quads_for_graph` (engine.rs:1698-1700), which reads `self.store` directly, one call per graph, outside any transaction and so cannot give the snapshot of 5.3; R1 adds `TripleStore::quads_for_graphs_snapshot(&[GraphName]) -> Vec<Quad>` in `src/store/` (in scope) that opens one transaction, or one mirror copy, for the whole set.

### 5.2 Premises

The dataset's data graphs (`list_dataset_graphs` minus `urn:system:reports:*`, the filter `validate_dataset` applies at routes.rs:7344-7350 and `infer_dataset` at :7775-7781); `entailment::reasoning_sources` (entailment.rs:479-494); the materialised `urn:entailment:{regime}:{dataset}` graph when the dataset is in materialize mode; the shape graphs. Model graphs are read-only for rules (never a patch target). This closes the gap the gate has today, where model graphs are absent: `compute_closure` seeds its set with the class itself and walks `rdfs:subClassOf` only over the selected graphs (`self.step(&node, RDFS_SUBCLASS, true, sel)`, src/shacl/view.rs:585-600), so without the model graph the closure is the singleton. Every graph is checked with the caller's rights: dataset graphs via the dataset permission (section 8.1), model graphs via `check_graph_read_access` or `conformance::model_graph_readable` — the two checks `POST /api/reasoning/materialize` applies (routes.rs:8672-8674).

**Entailment staleness.** `run_for_dataset_with` CLEARs the entailment graph before re-materialising (entailment.rs:164-171); if the last run failed the graph is empty and class-based rules under-fire. The report carries `"entailment": "stale"` when `dataset_entailment.last_run_at` (entailment.rs:42, :85) predates the newest commit touching the dataset.

### 5.3 Snapshot consistency

On a persistent store one `store.store().start_transaction()` is opened and every graph is read through `Transaction::quads_for_pattern` (oxigraph-0.5.9/src/store.rs:1254) — the `RawSource::Snapshot` pattern of the SHACL view (view.rs:189-199, the only `start_transaction()` call in `src/` today, followup-change-capture); when the accelerator has a clean copy, `mirror_full_copy()` (engine.rs:448) is used instead. On the memory backend there is no snapshot ("snapshots per call", view.rs:505), so the chase records `write_generation()` before and after seeding and marks the proposal `"consistent": false` on mismatch. The proposal records the generation (informational, process-local, JSON only) and the durable base marker of section 7.1.

### 5.4 Memory, time, and the bulk-revalidation workload

Seeding cost is **unmeasured**. Proxies: the in-memory bulk loader runs at ~0.5–0.9 Mt/s at the smaller tiers (docs/performance.md:779) — a Turtle-stream figure, not `bulk_insert_quads` of parsed quads — so 0.9M quads is roughly 1–2 s and 9M roughly 10–18 s, extrapolated, not measured; an inline rebuild of the accelerator at 1.7M triples held a query for ~29 s (src/store/parallel_mirror.rs:88-90; the accelerator holds two copies, :70). RAM: the accelerator budgets `BYTES_PER_TRIPLE_BOTH_COPIES = 1024` for two copies (parallel_mirror.rs:70); no single-copy figure exists.

**Per-request cap.** `OTS_REPAIR_MAX_QUADS` defaults to `detect_memory_limit_bytes()/8/512` (halving the two-copy assumption; the divisor 8 is the SHACL run-index budget's, view.rs:149-152) when the limit is detectable and to `DEFAULT_MAX_TRIPLES = 2_000_000` (parallel_mirror.rs:56) only when it is not. It is **never clamped upwards**: the accelerator's floor (`cap.clamp(DEFAULT_MAX_TRIPLES, ABSOLUTE_MAX_TRIPLES)`, parallel_mirror.rs:669-678) preserves one historical shared copy, but applied here it would raise a per-request allocation above what RAM says — in the shipped 4g container (docker-compose.yml:119) the formula gives 1 048 576 quads, and that is the cap. The count checked is the summed `graph_count_cached` (engine.rs:1782) of **every** premise graph — data, model, vocabulary, entailment and shape graphs — before seeding; over the cap the request is refused with 400 naming the variable and the count (the accelerator only warns and switches itself off, parallel_mirror.rs:475-481; refusal is new here). `scope.graphs` (section 8.1) bounds the seed; `scope.focus` bounds targets, not the seed.

**Aggregate cap.** The `expensive_semaphore` (capacity `available_parallelism/2`, mod.rs:227-231 — 12 on a 24-thread host) is not a memory budget: twelve concurrent sandboxes would be twelve times the cap, the OOM the accelerator's RAM-aware default exists to avoid ("rather than OOM-killing the process — the failure mode that flapped the container during a large seed", parallel_mirror.rs:68-70). The repair handler therefore takes a second, dedicated semaphore `OTS_REPAIR_CONCURRENCY` (default 1) *in addition to* the expensive permit, so resident sandbox memory is bounded by `concurrency × cap × 512 B` (≈ 512 MB in the 4g container at the default), and the seed loop checks the request deadline between graphs so a timed-out request abandons the copy at the next graph boundary rather than holding both permits until it completes.

**Bulk revalidation is a real, recurring workload.** Shapes and models are updated while instance data stays in place, so after a shape or model update every bound dataset must be revalidated whole — the SHACL Studio pipeline run, and, with this layer, the same run plus a chase. That is exactly the "many targets, bulk, Core-only" regime SHACL→SQL optimises, and the write gate already runs the same regime per Graph Store POST (gate.rs:55-67, :392, no changed-node scoping). The native engine's post-rebuild figure is 0.64–0.93 s over HTTP at 0.9M quads on the published mirror (docs/performance.md:657); at 9M the only figure is 118 s (performance.md:624, in the 2026-09 OTL in-process table, "Apple M-series laptop, release build", :613). That table is not labelled with the engine version or accelerator state — accelerator off follows from the 2M default cap (parallel_mirror.rs:56); "pre-rebuild" is asserted by the fit-assessment draft, not by the doc. docs/performance.md has no other SHACL figure at 9M (other docs were not exhaustively searched). The decision is therefore an experiment, not an argument.

### 5.5 The gating experiment for SHACL→SQL (deferred pending measurement)

**Harness.** `examples/scale_otl.rs` cannot produce the number as-is: positional arguments only (:117-125), phases unconditional, no `--skip-load`, no settle between the query phase (:170) and the SHACL phase (:191-202), and the JSON `shacl` object has no source label (:270). On a host whose detected budget covers 9M (≥ ~36.9 GB, below) the accelerator rebuild runs on a background thread and the harness races it; below that the accelerator is off and the phase measures the snapshot path. Required changes (in scope, `examples/`): a `--skip-load` that reuses an existing data dir (`scripts/scale_compare_http.py` already has `--skip-load` and `--settle`, :43-44, but no SHACL phase); an explicit `store.accelerator_tick()` loop (engine.rs:413) or settle until the mirror publishes; emitting `view.source_kind()`/`has_index()` into the JSON; a `--shacl-only` phase selector; and a second phase that seeds a `TripleStore::in_memory()` from the dataset's graphs by quad copy, then runs `shacl::validate` **on that sandbox** (where `DataView::new` takes `RawSource::Live` — no mirror, no snapshot, view.rs:189-199 — so the published-mirror figure does not apply), then a compiled chase, timing the three spans separately. The endpoint's cost is seed + chase + up to two sandbox validations, and nothing in the tree measures any of the three.

**Runs.** OTL at 0.9M and 9M quads, six property shapes, on the reference machine (54.9 GiB visible to Docker, performance.md:398) and in the shipped default container (`mem_limit: ${TRIPLESTORE_MEM_LIMIT:-4g}`, docker-compose.yml:119, where `ram_aware_default_max_triples` clamps to the 2M floor, parallel_mirror.rs:669-678, so the mirror is OFF at 9M). For 9M on the mirror the detected budget must be ≥ 9e6 × 1024 × 4 ≈ 36.9 GB or `OTS_PARALLEL_QUERY_MAX_TRIPLES` set explicitly. Each configuration: three runs, median.

**Record.** Per run: SHACL wall time, `source_kind` (mirror / snapshot / live), `has_index`, focus-node count, result count, peak RSS; the sandbox seed time, sandbox-validate time and chase time, with RSS, at both sizes; and, separately, the gate-sandbox cost — POST a 50-quad payload into a graph of 10⁵ and 10⁶ quads with one gating pipeline, reporting dump, parse and validate as separate spans (gate.rs:60-66, :367, :401-407).

**Decision thresholds** (proposed here, to be confirmed by the maintainer): SHACL→SQL stays deferred if the 9M whole-dataset run on the published mirror is ≤ 15 s (16–23× the 0.9M figure of 0.64–0.93 s, i.e. within ~2× of linear; ≤ 10 s if near-linear is the intended bar) *and* the snapshot-path run is ≤ 60 s. It is reopened as an active design item if the mirror run exceeds 60 s, or if the snapshot-path run — the one the default 4g container will take — exceeds 120 s (no better than the 118 s of unknown provenance). The gate measurement decides a separate, cheaper item first: changed-node scoping in the gate. For this note, the seed + sandbox-validate + chase measurement decides sync versus async (section 8.1), the default wall-clock budget (4.5) and the default cap.

## 6. Canonical repair

**Definition.** A repair is the set of proposed ops. It is *minimal* in the restricted-chase sense: every op is the consequence of an active trigger, no witness is minted when one exists, no add duplicates an existing quad, no merge is proposed twice; removing any op leaves some `Repair` rule unsatisfied. It is *deterministic* (4.7) and *explained* (every op carries rule, trigger bindings and premise quads).

**"Canonical" among several minimal repairs.** The chase makes choices only at EGDs (which term survives) and at head placement, both fixed by total orders (4.4, 4.6). Every other choice a violation would need — which value to drop for `sh:maxCount`, which member of `sh:in`, which disjunct of `sh:or`, which side of `sh:equals` — is **not made**: those constraints compile to `Report` rules, the trigger is counted, and the violation is left for the reviewer or the LLM step, unless an opt-in policy (`maxCount-keep-lexmin`, `closed-delete`, `datatype-relabel`) supplies a declared total order. The canonical repair is the unique fixpoint of the `Repair` rules under the declared orders and the computed stratification (replacement rules included, in their own strata), plus the list of what it deliberately did not touch.

**Conflicts between rules.** Two TGDs adding the same triple are harmless (set semantics; both justifications recorded). A replacement rule's retract is applied in its own stratum (3.4); stratification guarantees that no rule in a higher stratum produces the retracted predicate, and a retract that would remove a quad in `derived` is reported as a rule conflict, never applied. *Policy* deletions (`closed-delete`, `maxCount-keep-lexmin`, `Rewrite`) run in the last stratum after all additions and likewise never delete a quad in `derived`; the typed-null versus `closed-delete` interaction of 3.6 is the expected case of that rule. An EGD merge that creates a `maxCount` violation on the representative is residue. EGD conflicts (4.4 step 1) are reported, never resolved.

**Explicitly out of reach.** `sh:sparql` (all of `nen2660-relations` and `tests/fixtures/waalbrug/shapes-sparql.ttl`), custom components, `sh:expression`, `sh:pattern` and the other value-constraining constraints, `sh:not`, any non-predicate path, and any op whose subject or object is a blank node or a triple term (rdf_patch.rs:354-358, :103-111). Their share is reported as `report_only`/`unexpressible` counts. The blank-node share in the target corpora is *unmeasured*; the census is `SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } FILTER(isBlank(?s) || isBlank(?o)) } GROUP BY ?g` over each target dataset, plus the fraction of `shacl_validation_runs.report_json` results whose `focusNode` starts with `_:`.

## 7. The proposal artefact

### 7.1 RDF Patch

```
H id <urn:ots:proposal:5c1e…> .               # sha256(dataset ‖ base-commit ‖ base-sequence ‖ rules digest ‖ patch body)[..32]
H dataset <{base}/dataset/{id}> .
H base-commit <{base}/commit/{uuid}> .      # newest commit touching the dataset's graphs (list_commits, commit_log.rs:322)
H base-sequence "48123" .                    # change-capture sequence once it exists; absent until then
H rules <urn:…> .                            # content digest of the effective rule set
H engine "ots-chase/0.1" .
H complete "true" .
TX .
# rule=<urn:ots:rule:mincount:ex-BridgeShape:hasDeck> trigger=3f9a… focus=<http://…/bridge/17>
A <http://…/bridge/17> <http://…/hasDeck> <{base}/.well-known/genid/5c1e…> <http://…/graph/instances> .
A <{base}/.well-known/genid/5c1e…> <…#type> <http://…/Deck> <http://…/graph/instances> .
TC .
```

Every header is a function of the snapshot and the rules: the id is a content hash (not a uuid-v4 like commit ids, commit_log.rs:126), so the determinism claim of 4.7 and the byte-identity test of section 9 hold for the whole patch text; the process-local `write_generation` and `elapsed_ms` appear only in the JSON report. Headers are untyped strings and only `id` is consumed (rdf_patch.rs:299-308, :62-67), so the extra headers are backward compatible; `#` lines are skipped (:286-288), so per-line rule comments are legal and inert for any RDF Patch consumer. D lines precede A lines per graph, both sorted, matching `generate` (:485-496). The emitter is a new `generate_from_sets(headers, &[(target, from: &HashSet<String>, to: &HashSet<String>)])` with today's `generate` becoming a thin wrapper that builds its sets with `triples_of` (:442) — the existing test `generates_a_patch_from_two_graphs` (:740) keeps passing, and no cross-store diff is needed because the sandbox's before-set is collected at seeding and the after-set from the evaluator's bookkeeping. Before returning, the text is round-tripped through `rdf_patch::parse` so a proposal is appliable by construction.

**PROV-O per run.** Each chase run is a `prov:Activity`, written in the same form the commit log already uses (`a prov:Activity, ver:Commit`, src/commit_log.rs:177-178): `<proposal> a prov:Activity, ots:RepairProposal ; prov:used <base commit>, <rules graph> ; prov:generated <patch literal> ; prov:startedAtTime ; prov:wasAssociatedWith <actor>`. It is emitted in the JSON report always and stored with the proposal file when persisted (7.3); it enters the RDF store only through the commit an apply creates: `prov:wasInformedBy <proposal>` via `CommitRecord.metadata` (commit_log.rs:104, serialised as `ver:metadata` at :209).

### 7.2 Structured report (JSON, `"schema": 1`)

```json
{ "schema": 1, "proposal_id": "urn:ots:proposal:5c1e…", "dataset_id": "…", "status": "proposed",
  "base": { "commit": "…", "sequence": 48123, "write_generation": 4711, "consistent": true, "entailment": "fresh" },
  "rules": { "graphs": ["…"], "digest": "…", "compiled": 14, "authored": 2, "report_only": 9 },
  "summary": { "adds": 312, "deletes": 4, "merges": 6, "nulls": 118, "conflicts": 1, "unexpressible": 27,
               "unplaceable": 0, "rounds": 3, "exhausted": null, "elapsed_ms": 840 },
  "validation": { "before": {"violation": 340, "warning": 12}, "after": {"violation": 31, "warning": 12},
                  "residual": [ /* ValidationResult, capped at 1000 */ ] },
  "actions": [ { "op": "A", "graph": "…", "quad": "<s> <p> <o>", "rule": "…", "stratum": 1,
                 "confidence": "certain",
                 "trigger": { "bindings": {"this": "<…>"}, "premises": ["<s> a <Bridge> <g>"] },
                 "violation": { "focus_node": "…", "path": "…", "source_constraint": "sh:minCount 1" },
                 "explanation": "<…> needs at least one ex:hasDeck of class ex:Deck" } ],
  "merges": [ { "loser": "…", "winner": "…", "rule": "…", "mode": "same-as-only", "reason": "lexmin" } ],
  "conflicts": [], "report_only": [ { "rule": "…", "triggers": 40, "reason": "sh:maxCount undetermined" } ],
  "patch": "H id …", "actions_truncated": false }
```

`confidence` is `certain` (forced by the semantics), `policy` (a declared choice) or `heuristic` (reserved for LLM-produced actions). `validation` is present only when the request sets `validate: true` (default **false**): `before/after` come from `shacl::validate` (engine.rs:43) run twice inside the sandbox, where the view is `RawSource::Live` (view.rs:189-199) and the cost is unmeasured (5.5); with shape graphs copied in, `sh:sparql` constraints read `view.store`, which is the temp store, so "after" includes what the chase cannot repair. `source_constraint` stays the engine's free-form string until the additive `source_constraint_component` field lands (R0). Field names are the contract a review UI builds on; `actions` is capped at 10 000 inline and paged on the persisted resource (`?offset=&limit=`). Determinism tests compare the report with `summary.elapsed_ms` and `base.write_generation` removed.

### 7.3 Size, persistence, retention

The patch is never truncated (bounded by the ops budget); `complete: false` names the exhausted budget.

**Persistence is off the RDF store.** The first draft stored proposals as RDF in an unregistered `urn:system:repair-proposals` graph. That costs more than a one-graph recount: the `INSERT DATA` goes through `store.update` → `begin_write`, which calls `parallel_mirror.write_started()` and invalidates the query cache (engine.rs:430-434, again on guard drop :306-311), dirtying the accelerator so its next tick rebuilds **both** in-memory copies of the whole store — inline below `DEFAULT_INLINE_REBUILD_MAX_TRIPLES = 50_000`, otherwise on a background thread with queries falling back to RocksDB meanwhile, ~29 s at 1.7M (parallel_mirror.rs:84-92) — while `static_update_targets` (src/store/engine.rs:793-834) returns the one graph for `INSERT DATA` so `update` recounts only that graph (:769-772); and a multi-megabyte patch literal would land in `id2str`. Persisting a review artefact must not trigger a whole-store rebuild. Proposals are therefore written as files under `{data_dir}/repair-proposals/{dataset_id}/{proposal_id}.json` (report, PROV activity, status) and `.patch`, a sibling of `assets/` (src/main.rs:490-492) and `tantivy/` (:500-503). Consequences, stated rather than hidden: the directory is not backed up (`run_once_inner` writes only `rdf.nq.gz` and `auth.sqlite`, src/backup/mod.rs:146-208, and never walks `data_dir`) and not quarantined by recovery (`quarantine_store_files` moves only files matching `is_rocksdb_file`, src/store/recovery.rs:143-161; file names must avoid the `.log` suffix that :119-135 would classify as a RocksDB WAL) — a proposal is ephemeral, like the text index, and is recomputable from its base marker and rules. No new SQLite table (`pipeline_runs` lives in src/auth/db.rs, out of scope) and no store write.

Retention: newest 20 per dataset, older ones marked `expired` and deleted on insert, mirroring `pipeline_runs` pruning to `retention.clamp(1, 500)` (src/shacl_studio/store.rs:458); `OTS_REPAIR_PROPOSAL_TTL` (default 30 days) expires the rest on the next list or insert; dataset deletion removes the directory. States: `proposed → applied | rejected | superseded | expired`. `superseded` is set when an apply's precondition fails (8.3) **or** when a `GET` finds the newest commit touching the dataset's graphs (`list_commits`, commit_log.rs:322; the change-capture sequence once it exists) newer than the proposal's base marker — the response then also carries `"stale": true` so a reviewer sees it before attempting an apply.

**Never auto-applied**: the repair handler holds no write path; the only routes that reach the store are the patch route and the proposal `apply` resource, both requiring a writer's credentials and an explicit request.

### 7.4 Per-triple provenance, when change capture and 3.7 land

The brief asks for PROV-O per rule run now and RDF-star per derived triple when 3.7 lands. The per-run form is 7.1. The per-triple form is fixed here so the two phases agree:

- **Form.** For every `A` line of an *applied* repair, one named-reifier annotation: `<r> rdf:reifies <<( s p o )>> ; prov:wasGeneratedBy <proposal> ; ots:rule <rule iri> ; ots:trigger "3f9a…" ; prov:generatedAtTime "…"^^xsd:dateTime`, with `<r>` = `{base}/.well-known/genid/prov/{sha256(proposal_id ‖ quad in N-Quads)[..32]}` — content-derived like a null. A named reifier is the only form that is ground and idempotent under the delta path: `ground_update_delta` sends it through the `store.contains` de-duplication branch because a triple-term object is not `Term::BlankNode` (engine.rs:1073-1074, :1039), whereas the bare `<< s p o >>` shorthand mints a blank reifier and is counted +1 unconditionally (followup-rdf12 Q1). `DELETE DATA` can name an IRI reifier but never a blank one (`GroundQuadData`, spargebra-0.4.6/src/parser.rs:1439), so the inverse patch of an applied repair can remove its annotations.
- **Placement.** A per-dataset Provenance-role graph `urn:ots:repair-provenance:{dataset_id}`, the precedent being the property-states graph `urn:ots:property-states:{dataset_id}` registered with `GraphKind::Provenance` (src/property_states.rs:51-53, :163) and documented as "the data graph always carries the current value as a plain triple, so SPARQL, SHACL and reasoning see nothing new" (:1-9). Not the data graph: annotations there would be reasoning premises (`is_reasoning_source` admits `None | Instances | Model | Vocabulary | DomainValues | Linkset`, src/conformance.rs:70-81), be typed `rdfs:Resource` by rdfs4a, inflate per-graph counts, and — most important for this layer — be read by every user `?s ?p ?o`. Not `urn:system:commit-log`: unregistered, so invisible to non-admin `/sparql` scoping.
- **What today's code does with that graph.** Excluded from reasoning premises (conformance.rs:70-81) and from ICDD export (`urn:ots:` prefix and `GraphKind::Provenance` both skipped, src/containers/mod.rs:612-620). Three readers are role-blind and need stating: `materialized_datasets_for` re-runs the regime for any registered graph of a materialize-mode dataset (entailment.rs:100-108), so the annotations are written **in the same `update_targeted_delta` call and the same `after_write`** as the data patch (8.3 step 3) — one re-materialisation, not two; LDES `tracked` captures any registered non-private graph of a streamed dataset (src/ldes/store.rs:70-74) and `describe_entity` would print `<<( … )>>` into an N-Triples member body (src/ldes/capture.rs:94), so the graph is registered `private = 1`, which keeps it out of `tracked` at the cost of being readable only by principals who can write the dataset (src/auth/db.rs:4183-4192) — acceptable for repair provenance, stated as a trade-off; and the gate's `pipeline_covers_graph` does not consult roles (gate.rs:466), but `check_patch_gates` (8.3 step 2) validates only the graphs the patch names, and annotations are server-generated, not patch lines, so they are never gated.
- **Cost.** k+2 quads per annotated statement (followup-rdf12 Q2); for a 312-add proposal, ~1 250 quads. Storage per quad is *unmeasured* in the tree.

## 8. Endpoint

### 8.1 `POST /api/datasets/:dataset_id/repair`

Mounted in `shacl_routes` beside `/validate` and `/infer` (mod.rs:1374-1414), which carry `endpoint_acl_guard` and `require_auth` (:1410-1414) — so an anonymous call gets 401 from the middleware instead of the patch route's 500 — and no `GovernorLayer` (the `sparql_rate_conf = make_rate_conf(1, 40)` tier at mod.rs:811 does not apply); admission control is the `expensive_semaphore` plus the dedicated repair semaphore (5.4).

Request (JSON; `Accept: application/rdf-patch` returns the patch text only):

```json
{ "rules": ["<rules graph iri>"],
  "derive": { "from_shapes": true, "from_owl": true, "entailment_rules": false },
  "policies": ["closed-delete", "maxCount-keep-lexmin", "datatype-relabel"],
  "shapes_graph": "<iri>",
  "scope": { "graphs": ["…"], "focus": ["<iri>"] },
  "budget": { "rounds": 100, "nulls": 10000, "ops": 50000, "timeout_secs": 30 },
  "validate": false, "persist": false }
```

`scope.graphs` restricts the *data* graphs seeded (model, entailment and shape graphs are always seeded whole); `scope.focus` restricts *targets* — it compiles to a `VALUES ?this { … }` on every rule — and does not shrink the seed, because no neighbourhood-extraction rule is defined and a partial seed would make guards fire falsely.

**Auth: write permission** (`can_write_dataset`, 403 "Write access required"), mirroring `infer_dataset` (routes.rs:7760-7766) and the patch route (:527-533), not `validate_dataset`'s read check (:7285-7291). Reason: `D` lines quote existing triples, and graphs flagged `private` are in the readable set only for principals who can write the owning dataset (src/auth/db.rs:4183-4192; models.rs:679-681); `can_access_dataset` is dataset-level, so a read-gated proposal would leak private-graph content. **Read-scoped API tokens.** `enforce_write_scope_for_mutation` treats every POST except a `/sparql` query as a mutation and rejects a read-scoped token with 403 before the handler (src/auth/middleware.rs:601-624); a proposal is read-only in effect but write-gated by the private-graph argument above, so the two agree and no exemption is added — widening the `/sparql` carve-out (middleware.rs:610-618) is a `src/auth` edit and outside this programme regardless. Consequence, stated: the assistant channel of section 9 must call this endpoint under a JWT session (`write_access: true`, middleware.rs:192) or a write-scoped token, and a viewer-only principal cannot obtain a proposal even for a dataset with no private graphs. Status codes copy the neighbours: 404 "Dataset not found" (:7279-7283); 403 for denial, because `audit_forbidden` records only 403 (middleware.rs:477); 400 for an unstratifiable or unparsable rule set, no resolvable shapes (`NO_SHAPES_GRAPH_MSG`, :7771-7772), or a premise set over `OTS_REPAIR_MAX_QUADS`; 503 "Server overloaded" from either semaphore (:7273-7278); 200 with `exhausted` on budget exhaustion. A successful proposal is unaudited (no read/proposal event type exists and adding one is a `src/auth/audit.rs` edit).

Execution: both permits, then `spawn_blocking` under `tokio::time::timeout(budget.timeout_secs)`, with the chase checking its wall-clock budget between graphs during seeding and between rules during evaluation, returning partial results rather than being killed — `tokio::time::timeout` cannot stop a blocking task, so the in-task deadline is the real mechanism and the outer timeout only bounds the response. Deliberately not inline on the async handler as `validate_dataset` runs today (routes.rs:7355). Synchronous in R1–R2; an async job (202 + `GET …/repair/jobs/:id`) only if the 5.5 measurement shows 9M-quad datasets cannot finish within the timeout — no job framework exists.

### 8.2 Proposal resources

`GET /api/datasets/:id/repair/proposals` (list), `GET …/proposals/:pid` (report + patch, paged actions, `stale`), `POST …/proposals/:pid/reject`, `POST …/proposals/:pid/apply` — all write permission. Denials are 403 → `PermissionDenied` automatically; an apply logs `SparqlUpdate/Success` with `details = {graphs, message, proposal}` exactly as routes.rs:811-824 does, the only "data changed" audit variant. The `apply` resource reads the patch from the proposal file, so a proposal larger than the 8 MiB body limit (mod.rs:2010) is applied here and only here.

### 8.3 The apply path and the gate it adds

`POST /api/datasets/:id/patch` is unchanged by default; `apply_patch_handler` is refactored into `rdf_patch::apply_parsed(state, dataset, patch, actor, options)` that both routes call (src/rdf_patch.rs is outside the listed scope; section 10). Two opt-in query parameters, and the same options on `proposals/:pid/apply`:

1. **`?if-base-commit=<iri>`** (or `If-Match`, the precedent being the data-model draft ETag at src/data_models/handlers.rs:898-911): compared with the newest commit touching any graph named in the patch (`list_commits`, commit_log.rs:322); mismatch → `AppError::Conflict` 409 (error.rs:75), and a persisted proposal becomes `superseded`. Until change capture lands this is best-effort in two ways: it misses paths that record no commit (validate-and-commit, RML, LDP, seed loads), and it misses the window between a data write and its commit record (rdf_patch.rs:598 versus :614; section 1.3). Once CC lands the check becomes `if-base-sequence` against the durable cursor, which closes both only if the sequence is assigned inside the write primitive.
2. **`?validate=true`**: a new `gate::check_patch_gates(ctx, &patch)` in src/shacl_studio/gate.rs (in scope): for each touched graph, seed a temp store by quad copy (labels kept), apply the patch's `to_sparql_update` text to it, copy the discovered shape graphs, and run `evaluate_gates` over the **set** of touched graphs. Failure → 422 `ValidationFailed` with the same body the Graph Store gate returns (error.rs:57-73); infrastructure failure fails closed (`gate_error`, gate.rs:79). Cost is O(Σ|touched graphs|) seed plus a full validation — unmeasured, same order as today's gate per POST.
3. **Apply.** Because a patch is ground, call `update_targeted_delta` (engine.rs:942-947) instead of `update`: `ground_update_delta` simulates every operation in order (engine.rs:1045-1058), so a multi-block patch qualifies, the count index is adjusted exactly (engine.rs:969-979) and the text index receives the delta (the routes.rs:793-795 pattern) instead of a per-graph recount (engine.rs:769-774). Today's patch apply pays the recount. When 7.4 is live, the provenance annotations are part of this same call.
4. **TOCTOU.** No per-dataset lock exists anywhere (followup-patch-and-diff Q5); a write between gate and apply is undetected unless the commit precondition catches it. R2 adds a per-dataset `DashMap<String, Arc<Mutex<()>>>` (dashmap is already a dependency, Cargo.toml:317) hung off `TripleStore` (src/store/, in scope; `AppState` lives in mod.rs), held across steps 1–3 by the two patch entry points; other write paths do not take it in this phase, so the guarantee is "no two gated applies interleave", stated as such.
5. **Fan-out**, unchanged: `ldes::capture::before/after` (capture.rs:101, :122; two O(graph) scans per tracked graph), `entailment::after_write` (non-additive → `after_write_kind(additive=false)` → CLEAR and full re-materialisation, entailment.rs:184-186, :197-225, :164-171 — a repair patch has `D` lines, so the extend path is never taken), text-index sync. Response fields stay as today (:630-637).
6. **Commit and provenance.** R2 keeps `CommitKind::Sparql` (as :617) with message `Repair {proposal_id}: +a −d` and counts from the measured delta of step 3, and builds the record through `insert_commit` with `CommitRecord.metadata` = `{"proposal", "produced_by", "base_commit", "rules"}` (commit_log.rs:104, :209). A `CommitKind::Repair` variant serialising as `"repair"` (kebab-case, commit_log.rs:25-39) is additive and listed as a touchpoint. Per-triple provenance waits for change capture (provenance.rs:12-14), in the named-reifier form of 7.4.

### 8.4 Documentation

The new routes and parameters go into `src/server/openapi.rs` (which has `/infer` at :1526 and no patch route today) and `docs/api-reference.md` (which mentions none of `/validate`, `/patch`, `/infer`).

## 9. Failure modes, limits, and the LLM step afterwards

| Failure | Behaviour |
|---|---|
| Non-terminating rule set | budget exhaustion; partial proposal with `exhausted`; offending rules named by trigger count |
| Unstratifiable negation or retract cycle / unguarded existential | 400 at rule load naming the rules |
| EGD conflict | recorded in `conflicts`; no merge |
| Sandbox over cap | 400 naming `OTS_REPAIR_MAX_QUADS` |
| Repair concurrency exhausted | 503 "Server overloaded" |
| Base moved before apply | 409 when the precondition is supplied; `stale: true` on GET; unrecorded writes and the write-to-commit window invisible until change capture |
| Gate rejects at apply | 422 with report; proposal stays `proposed` |
| Existing blank-node triples | never proposed for deletion; merges on a bnode subject are residue (`unexpressible`) |
| Triple terms (RDF 1.2) | `generate` output cannot be tokenised by `parse` (:103-111); lines omitted, `complete: false` |
| Multi-graph placement undetermined | `unplaceable` |
| Stale or empty entailment graph | `"entailment": "stale"`; class rules may under-fire |
| Memory-backend seed inconsistency | `"consistent": false` |
| `Rewrite` merge side effects | LDES tombstone for the loser (capture.rs:145-161), text-index document dropped; `SameAsOnly` avoids both |
| Authored rules with `Union` scope | may propose triples the per-graph validator still flags; documented |
| Witness would violate a sibling constraint or a closed shape | compiled to Report (3.6); never minted |
| Proposal larger than the 8 MiB body limit | appliable only through `proposals/:pid/apply` |

Limits: no repair of anything SPARQL-based; no literal nulls; no choice-making without an opt-in policy; no delta validation (the sandbox is seeded whole); no async job; no RDF-star annotations consumed, and none produced until 7.4; proposals are not backed up; a successful proposal is not audited.

**Test plan** (beyond the unit tests of R0): a two-graph fixture asserting head placement, per-graph value confinement and union-scoped typing (the fixture at engine.rs:2299-2317 is almost that); HTTP tests for 401 (anonymous), 403 (viewer, read-scoped token), 404, 400 (unstratifiable; retract cycle), 200-partial (budget), 503 (repair semaphore); 422 and 409 on the gated apply; byte-identical patch text and report-minus-volatile-fields across two runs; idempotence of applying a proposal twice; a replacement rule whose output feeds a key EGD; a `Rewrite` merge followed by `after_write` under `Narrow` and `Off`.

**What the LLM step becomes.** After this layer the LLM's input is the *residual*: `validation.after.residual` plus `report_only` and `conflicts` — exactly the violations no deterministic rule could or would repair (`sh:in` members, `sh:pattern` fixes, ill-formed literals, disjunct choices). Its output is constrained to the same artefact: new `ots:Rule` definitions (which run through the chase, so they are stratified, budgeted and explained) or a patch, both entering the proposal store with `confidence: "heuristic"` and passing the same gated apply. **Guard on the reverse direction.** A heuristic rule set is run with its own, smaller budget (rounds 10, nulls 1 000, ops 5 000 by default; a heuristic rule that exhausts it is dropped from the proposal and reported, not returned as a partial repair), may not carry `ots:destructive true`, `ots:retract` or `ots:mergeMode ots:Rewrite`, is rejected at load (400, returned to the assistant as its next residual) if unstratifiable or if it names a predicate absent from the premises, and is never written into a shapes graph except by a human `save`. This keeps the Spark contract that the model never touches the store (docs/spark.md:225) while giving it a legitimate proposal channel; `ShaclAssistRequest` gains an optional `residual` field (additive, llm_sparql.rs:470-488), and the assistant's call to `/repair` runs under the caller's write-scoped credential (8.1). Whether to build it before telemetry shows the residue's share is an open question.

## 10. Phasing, effort, touchpoints, open questions

| Phase | Content | Effort | Depends on |
|---|---|---|---|
| CC | Per-quad change capture with a durable sequence assigned inside the write primitive (the P2-4 core; the maintainer's first deliverable) — designed in the delta-versioning note, referenced here as the base marker and precondition | separate item | — |
| R0 | `load_shapes`/`load_rules` → `pub(crate)`; `generate_from_sets` with `generate` as wrapper; additive `source_constraint_component` on `ValidationResult` (`skip_serializing_if`, wired through the `mk` closure at constraints.rs:147 and the other production construction sites — constraints.rs:472, :495, :519, gate.rs:87; the two under `#[cfg(test)]` are unaffected); the sibling-pair census of 3.6 over the vendored corpus; two tests in `tests/shacl_rules_conformance.rs` — unguarded bnode head yields 100 × focus nodes, guarded head yields 1 after two `infer()` calls; the `scale_otl.rs` changes and the measurement of 5.5 | 1.5 wk (estimate) | — |
| R1 | `src/repair/`: vocabulary loader, stratifier (negation and retract edges), naive evaluator, null minting, union-find, budgets, repair semaphore; compiler from SHACL Core and OWL; `quads_for_graphs_snapshot` in `src/store/`; sandbox seeding from a transaction snapshot or mirror copy; `POST …/repair` synchronous with patch + report + optional before/after validation; OpenAPI and docs | 2.5–3 wk (estimate) | R0 |
| R2 | `check_patch_gates`, `?validate=true`, `?if-base-commit`/`if-base-sequence`, `apply_parsed` with `update_targeted_delta` and the per-dataset lock, `CommitRecord.metadata` provenance, optional `CommitKind::Repair` | 1.5 wk (estimate) | R1, CC for the durable precondition |
| R3 | File-backed proposal persistence and resources, staleness on GET, opt-in policies, semi-naive evaluation, residual hand-off to the SHACL assistant with the heuristic-rule guard; per-triple provenance graph (7.4) once CC and 3.7 are in; async job only if 5.5 warrants | 1.5 wk (estimate) | R2 |

Total 7–7.5 weeks against the brief's 6–8 for P2-3. The brief lists P2-1 and P1-2 as dependencies; this design needs neither the analytical mirror (the sandbox is an in-memory oxigraph store) nor anything beyond the typed `Constraint` enum and the gate pattern that exist in the tree today. Its true dependency is CC.

**Performance constraint (no benchmark in docs/performance.md may get >20 % slower).** Nothing here runs unless requested, and persistence no longer writes to the store, so the expected delta on every published row is zero; that is proven, not assumed, by re-running before R2 merges: the SHACL rows (in-process 0.9M/9M, performance.md:624; over HTTP, :657) because `check_patch_gates` shares gate code, and the 4-writers-plus-4-readers rows (:625, :655) because the per-dataset lock and `apply_parsed` sit beside the write path — the lock is taken only by the two patch entry points, and the writers in that phase do not use them, so any movement there is a bug. The patch route itself has no published row; R2 adds one (apply of a 1 000-line patch at 0.9M) so the `update_targeted_delta` change is measured rather than argued.

**Files outside the listed implementation scope that this design touches** (each needs explicit approval): `src/server/mod.rs` (route mounting, repair semaphore in `AppState`), `src/rdf_patch.rs` (`apply_parsed`, `generate_from_sets`, `update_targeted_delta`), `src/commit_log.rs` (optional `CommitKind::Repair`), `src/main.rs` (the `repair-proposals/` directory beside `assets/`), and the new top-level `src/repair/`. Not touched: `src/auth/*` (no audit variant, no table, no middleware exemption, no new `GraphKind`), frontend, dependencies (union-find is hand-written; dashmap exists).

**Open questions**

1. Skolem base: `DEFAULT_SKOLEM_BASE` or `{base_url}`; whether `/.well-known/genid/` should dereference.
2. The engine's implicit-class ASK (engine.rs:455-460) and the per-graph `targetClass` closure (engine.rs:1196-1217): widen either to the model graphs (changes validation results for `nen2660-imbor`) or leave both and let only the compiler see those targets — the compiler must match whichever the engine does.
3. Sandbox cap default, default wall-clock budget and sync-vs-async: settled by the seed + sandbox-validate + chase measurement in 5.5.
4. Proposal files under `--data-dir` (as designed: not backed up, not quarantined) versus RDF in the store (whole-store accelerator rebuild per persist, 7.3) versus the file store's `ObjectStore` (backed by `assets/`, which would put proposals behind the asset API's own permissions).
5. `CommitKind::Repair` now or `Sparql` plus metadata.
6. Whether `Rewrite` should be allowed at all in R1, or only null→live substitution.
7. Whether `?validate=true` should become the default on `POST /patch` in a later major version — today's contract is ungated and changing it is a behaviour change this phase forbids.
8. Per-graph confinement is incidental (followup-shacl-internals Q5); if the engine's rule changes, the compiler's `PerGraph` default must follow — pin both with the two-graph fixture.
9. `datatype-relabel` on by default: safe by construction, but it changes literal identity and every query that matched the string form.
10. Whether the repair-provenance graph (7.4) should be `private` (keeps it out of LDES, hides it from viewers) or public with an LDES role check added in `src/ldes/store.rs` (in scope, but a behaviour change for existing Provenance-role graphs).
11. The SHACL→SQL decision itself: deferred pending the 5.5 experiment, with the thresholds above to be confirmed; if reopened, it rides on a SPARQL-subset→SQL translator rather than a separate compiler, and this layer is unaffected either way.
