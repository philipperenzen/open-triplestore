# Repair proposals

Validation tells you what is wrong with a dataset. The repair layer proposes
how to fix the part of it that the rules actually determine, and says why
for every line. It never changes the data itself: it computes a **proposal**
— an [RDF Patch](https://afs.github.io/rdf-delta/rdf-patch.html) plus a
report — in a throwaway copy of the dataset, and a writer reviews it and
applies it, through the dataset's write gates, like any other write.

```
validate → propose (POST …/repair) → review → apply (gated) → validate
```

The design, with its reasoning and the open questions it settled, is
`docs/notes/repair-layer-design.md` in the repository.

---

## What it proposes, and what it leaves alone

The rules come from three places, all in one run:

- **Compiled from the dataset's SHACL Core shapes** (the same shapes
  `POST …/validate` uses), for the constraints a shape *determines*:

  | Constraint | Proposal | Confidence |
  |---|---|---|
  | `sh:hasValue v` | add `focus path v` | certain |
  | `sh:class C` | add `value rdf:type C` (IRI values) | certain |
  | `sh:minCount n`, when an IRI value can satisfy every sibling constraint (`sh:class`, `sh:nodeKind sh:IRI`, `sh:node`, …) | mint the missing values as named placeholders ("nulls"), typed by `sh:class` | certain |
  | `sh:minCount 1` with a one-member `sh:in` | add that member | certain |
  | `sh:node`, `sh:and`, nested `sh:property` | the inner shape's constraints, for the values | as inner |
  | `sh:datatype` | relabel `"lex"^^other` as `"lex"^^dt` when `lex` is valid for `dt` | policy, opt-in `datatype-relabel` |
  | `sh:closed` | delete the triples it does not allow (never `rdf:type`) | policy, opt-in `closed-delete` |
  | `sh:maxCount n` | keep the `n` smallest values in N-Triples order, delete the rest | policy, opt-in `maxCount-keep-lexmin` |

  Everything else — `sh:in` with several members, `sh:pattern`, lengths and
  ranges, `sh:languageIn`, `sh:nodeKind`, property pairs, `sh:or` /
  `sh:xone` / `sh:not`, qualified shapes, non-predicate paths, a
  `sh:minCount` whose value must be a literal, any shape whose severity is
  not `sh:Violation` — is **report-only**: listed in the report with the
  reason, and (when the run validates) the number of results it accounts
  for. SPARQL constraints, constraint components and `sh:expression` are out
  of reach: they have no head to invert.

- **Compiled from OWL axioms** among the dataset's reasoning sources:
  `owl:hasKey`, `owl:FunctionalProperty` and `owl:InverseFunctionalProperty`
  merge two terms (an `owl:sameAs` line); `owl:someValuesFrom` and
  `owl:minCardinality 1` restrictions mint a witness. `rdfs:domain`,
  `rdfs:range` and `rdfs:subClassOf` only with `derive.entailment_rules`
  (a dataset that materialises entailments derives them already).

- **Authored** as `ots:Rule` resources, in a rules graph named in the
  request or in one of the dataset's shapes graphs (below).

What a proposal never does: delete a blank-node triple (no patch line can
name one), choose between values without a declared policy, or touch a
model, shapes, entailment, system or provenance graph — heads land only in
the dataset's own data graphs.

---

## Asking for a proposal

```bash
curl -X POST http://localhost:7878/api/datasets/<id>/repair \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: application/json' \
     -d '{ "validate": true, "persist": true }'
```

Needs **write** access to the dataset: a proposal quotes the triples it would
delete, and a graph a dataset holds as private is readable only by its
writers. A read-scoped API token is refused (any POST is a mutation to the
token check).

| Field | Default | Meaning |
|---|---|---|
| `rules` | `[]` | Graphs holding `ots:Rule`s (each one the caller may read). |
| `derive.from_shapes` | `true` | Compile the dataset's SHACL Core shapes. |
| `derive.from_owl` | `true` | Compile OWL axioms among the premises. |
| `derive.entailment_rules` | `false` | Also `rdfs:domain` / `rdfs:range` / `rdfs:subClassOf`. |
| `derive.shacl_rules` | `false` | Import the shapes graphs' SHACL-AF `sh:rule`s (a template blank node becomes a null, so an existential rule converges). |
| `policies` | `[]` | `closed-delete`, `maxCount-keep-lexmin`, `datatype-relabel`. |
| `shapes_graph` | the dataset's | Use this shapes graph instead. |
| `scope.graphs` | all | Copy only these dataset graphs (model, entailment and shape graphs are always copied whole). |
| `scope.focus` | all | Limit every rule's focus node (`?this`) to these IRIs. Rules without a `?this` do not run. |
| `budget.rounds` | 100 (cap 1000) | Rounds per stratum. |
| `budget.nulls` | 10 000 (cap 1 000 000) | Nulls minted. |
| `budget.ops` | 50 000 (cap 500 000) | Lines proposed. |
| `budget.timeout_secs` | the query timeout (cap 240) | Wall clock, checked while copying and between rules. |
| `validate` | `false` | Validate the copy before and after: counts by severity and the residual results. |
| `persist` | `false` | Keep the proposal for review and apply. |
| `semi_naive` | `true` | `false` re-evaluates every rule in full each round. |
| `heuristic_rules` | — | Turtle `ots:Rule`s to run *instead of* every other rule, under the assistant's restrictions (below). |

`Accept: application/rdf-patch` answers with the patch text alone.

A budget that runs out is **not an error**: the answer is `200` with
`summary.exhausted` set (`rounds`, `nulls`, `ops` or `time`) and
`H complete "false"` in the patch. Every line in it came from a rule that
fired against the state at that point, so it is a valid partial repair.

| Status | When |
|---|---|
| 400 | A rule does not parse or breaks the rule checker; the rule set cannot be stratified (a cycle through negation or a retract, naming the rules); no shapes graph (as for `/validate`); an unknown policy; the graphs to copy hold more than `OTS_REPAIR_MAX_QUADS` quads. |
| 401 / 403 / 404 | No token; no write access (or a named graph the caller may not read); no such dataset, or one the caller cannot see. |
| 503 | `Server overloaded`: a repair is already running (`OTS_REPAIR_CONCURRENCY`, default 1); or the time budget ran out while copying. |

### The patch

```
H id <urn:ots:proposal:5c1e…> .
H dataset <https://data.example.org/dataset/bridges> .
H base-commit <https://data.example.org/commit/…> .
H base-sequence "48123" .
H rules <urn:ots:rules:…> .
H engine "ots-chase/0.1" .
H complete "true" .
TX .
# rule=<urn:ots:rule:mincount:…:BridgeShape:hasDeck> trigger=3f9a… focus=<…/bridge/17>
A <…/bridge/17> <…/hasDeck> <https://data.example.org/.well-known/genid/5c1e…> <…/instances> .
A <https://data.example.org/.well-known/genid/5c1e…> <…#type> <…/Deck> <…/instances> .
TC .
```

Every header is derived from the data and the rules: the same dataset state
and rule set give byte-identical patch text, and the id is a content hash.
`base-commit` is the newest commit touching the dataset's graphs;
`base-sequence` / `base-epoch` the change log's position when change capture
is on (`OTS_CHANGE_CAPTURE=on`). `#` lines name the rule and trigger behind
each line and are ignored by every RDF Patch reader. Placeholder values
("nulls") are IRIs under `{base}/.well-known/genid/`, derived from the rule
and the focus node, so a second run mints the same IRI and never a second
placeholder.

### The report

| Field | Meaning |
|---|---|
| `proposal_id`, `status`, `partial` | `partial: true` when some dataset or shapes graph was left out because the caller may not read it. |
| `base` | `graphs` (the dataset graphs the run read), `commit`, `sequence`, `epoch`, `consistent`, `entailment` (`fresh` / `stale` / `off` / `withheld`), `source`, `quads`, and `write_generation` (process-local). The staleness check and the apply's base check are about `graphs`. |
| `rules` | `digest`; counts `compiled` / `authored` / `shacl_af` / `heuristic` / `report_only`; `strata`; every rule's `definitions`; the same as `turtle`, to save and edit; `skipped` targets. |
| `summary` | `adds`, `deletes`, `merges`, `nulls`, `conflicts`, `unexpressible`, `unplaceable`, `rounds`, `exhausted`, `elapsed_ms`. |
| `validation` | With `validate: true`: `before` / `after` by severity, `residual` (up to 1000 results). |
| `actions` | One per patch line (up to 10 000 inline): `op`, `graph`, `quad`, `rule`, `stratum`, `confidence`, `trigger` (`id`, `bindings`, `premises`), `violation` (the shape, path and constraint it answers), `explanation`. |
| `merges`, `conflicts`, `unplaceable`, `unexpressible`, `report_only`, `per_rule` | What the chase merged, refused, could not place or write, left alone, and how often each rule fired. |
| `prov` | The run as a PROV-O activity (Turtle). |
| `patch` | The patch text. |

Two runs over one dataset state agree on everything but
`summary.elapsed_ms`, `base.write_generation` and `prov` (its start time).

---

## Writing rules

A rule is a SPARQL `CONSTRUCT { head } WHERE { body }` with the
`https://opentriplestore.org/ns#` (`ots:`) terms around it:

```turtle
@prefix ots: <https://opentriplestore.org/ns#> .

<https://data.example.org/rules/deck> a ots:Rule ;
  ots:construct """PREFIX ex: <http://example.org/>
    CONSTRUCT { ?this ex:hasDeck ?w . ?w a ex:Deck }
    WHERE { GRAPH ?g { ?this a ex:Bridge } }""" ;
  ots:nulls ( "w" ) ;
  ots:graphScope ots:PerGraph ;
  ots:message "{?this} needs a deck" .
```

| Term | Meaning |
|---|---|
| `ots:construct` (or `sh:construct`) | The rule. The body may use basic graph patterns, `FILTER`, `BIND`, `VALUES`, `FILTER [NOT] EXISTS`, `MINUS` and `GRAPH`; no `OPTIONAL`, `UNION`, `SERVICE`, aggregates or property paths (a path may appear inside `FILTER NOT EXISTS`). |
| `ots:nulls ( "w" )` | Head variables the body does not bind: a placeholder IRI is minted for each. A blank node in the head is refused — declare it here. |
| `ots:equate ( "x" "y" )` | An equality rule: the head is empty and the two terms are merged. A live IRI beats a placeholder; between two IRIs the smaller N-Triples form wins. Literals, terms asserted `owl:differentFrom` (or in one `owl:AllDifferent`), and terms of disjoint classes are never merged: that is a *conflict*. |
| `ots:mergeMode` | `ots:SameAsOnly` (default): one `loser owl:sameAs winner` line. `ots:Rewrite`: every triple of the loser is moved to the winner (destructive; runs last). |
| `ots:retract "…"` | A template of triples the rule deletes when it fires: a replacement rule (unit conversion, relabelling). |
| `ots:graphScope` | `ots:Union` (default for authored rules): each body triple may match in any premise graph. `ots:PerGraph`: the top-level triples match in one graph. |
| `ots:targetGraph` | Where head triples land; default `ots:FocusGraph` — the graph of the subject's `rdf:type` triple in the body, else of the first body triple binding it. |
| `ots:priority` | Order within a stratum (like `sh:order`). |
| `ots:policy ots:Report` | The rule fires and is counted, and proposes nothing. |
| `ots:confidence` | `"certain"` (default), `"policy"`, `"heuristic"`. |
| `ots:message` | The explanation; `{?var}` is replaced by the binding. |
| `ots:derivedFrom`, `ots:destructive`, `sh:deactivated`, `sh:prefixes` | Provenance, a flag for reviewers, the off switch, prefix declarations. |

Each rule's head is guarded: it fires only when its head is not already
there (the *restricted* chase), so a second run over an applied proposal
proposes nothing. Rules are ordered into strata from what they read, negate,
produce and delete; a rule set that would both feed and negate (or delete)
one predicate through a cycle is refused with `400`.

The compiled set of a run is in `rules.turtle`: save it as a rules graph,
edit it, and pass the graph in `rules` (with `derive.from_shapes: false`) to
run your version instead.

---

## Reviewing and applying

With `persist: true` the proposal is kept for review:

| Method | Path | |
|---|---|---|
| `GET` | `/api/datasets/{id}/repair/proposals` | The kept proposals, newest first, with `status` and `stale`. |
| `GET` | `/api/datasets/{id}/repair/proposals/{pid}` | Report, patch, a page of actions (`?offset=&limit=`), `stale`. `Accept: application/rdf-patch` for the patch. |
| `POST` | `/api/datasets/{id}/repair/proposals/{pid}/apply` | Apply it (below). |
| `POST` | `/api/datasets/{id}/repair/proposals/{pid}/reject` | Reject it. |

States: `proposed` → `applied` | `rejected` | `superseded` | `expired`. A
proposal whose dataset changed after it was computed is `superseded` (the
change log's sequence when capture is on, else the newest commit) and a
`GET` says `stale: true`. Proposals are files under
`{data-dir}/repair-proposals/`: the newest 20 per dataset are kept, older
ones and anything past `OTS_REPAIR_PROPOSAL_TTL` days (default 30) expire.
They are not in backups — a proposal can always be computed again.

**Nothing is ever applied automatically.**

### Applying a proposal

```bash
curl -X POST http://localhost:7878/api/datasets/<id>/repair/proposals/<pid>/apply \
     -H 'Authorization: Bearer <token>'
```

Needs write access to the dataset. The apply, in order:

1. takes the dataset's patch lock — `POST /api/datasets/{id}/patch` takes
   the same one, so two applies never interleave between check and write;
2. checks the proposal's base: its change-log sequence when it has one and
   the log is still in the same epoch, else its base commit. If a graph the
   run read (`base.graphs`) changed since, the answer is `409`
   (`"error": "stale_base"`) and the proposal becomes `superseded`:
   compute it again;
3. runs the write gates of every graph it touches over what the graph would
   hold after it — the gates a Graph Store write to that graph passes
   (`shacl_on_write` shapes, Studio pipelines that gate writes,
   validation-layer bindings). A refusal is `422` with the validation
   report, and the proposal stays `proposed`;
4. writes the patch as one ground update: the count and text indexes take
   the exact change, a materialised entailment is refreshed and an LDES
   stream records the change, as for any other write;
5. records a commit (`GET /api/datasets/{id}/commits`) whose
   `metadata.repair` names the proposal, the engine, the base and the rules
   digest, and marks the proposal `applied` with that commit. With change
   capture on, the change-log rows of the write carry the same commit. The
   apply is audited as a SPARQL update.

The answer is `{applied, proposal_id, status, commit, added, removed,
graphs}`. A proposal that is not `proposed` is `409`
(`"error": "not_proposed"`), so applying twice changes nothing. The patch is
read on the server, so a proposal larger than the 8 MiB request limit is
applied here.

### The same checks on the patch route

`POST /api/datasets/{id}/patch` takes the base check as two opt-in
preconditions, for a patch you apply yourself (a downloaded proposal, or any
other). Both are about the dataset's graphs, the ones a proposal's base was
computed over:

| Option | |
|---|---|
| `?if-base-commit=<iri>` or `If-Match: "<iri>"` | `409` unless `<iri>` (or the bare commit id) is still the newest commit touching one of the dataset's graphs. An empty value means no commit has touched them yet. |
| `?if-base-sequence=<n>&if-base-epoch=<e>` | `409` if a change-log row after `n` touches one of the dataset's graphs. This also sees writes that record no commit. Needs `OTS_CHANGE_CAPTURE=on` (`400` without). |

A `409` body is `{"error": "stale_base", "message", "precondition",
"current"}`, where `current` is the newest commit or the log's sequence and
epoch. A proposal's `H base-commit`, `H base-sequence` and `H base-epoch`
headers are the values to pass. For a proposal whose run left graphs out
(`scope.graphs`, or graphs the caller may not read), the apply route checks
exactly the graphs it read and is the one to use. Without an option the route
applies a patch exactly as it always has.

### The assistant

SHACL Studio's assistant (`POST /api/llm/shacl`) takes
`"task": "repair"` with a `dataset_id`. It sends the model the *residual*
of a deterministic run: the results no rule repaired (at most 50), the
report-only constraints with their reasons, and the conflicts. Pass
`residual` yourself to skip that run. The model may only answer with
`ots:Rule`s. They run through the same chase, on their own (the
deterministic rules are not part of their proposal, so it can be reviewed
apart), as heuristic rules:

- with a smaller budget (10 rounds, 1 000 nulls, 5 000 lines). A heuristic
  rule that exhausts it is dropped and named in `rules.dropped`;
- never destructive: no `ots:retract`, no `ots:destructive`, no
  `ots:mergeMode ots:Rewrite`;
- only over predicates the premises already use.

Their proposal is kept with confidence `heuristic` for review and applied
like any other. A rule set that does not load or is refused by those checks
comes back as `rejected`, with no proposal: the message is the next thing
to answer. Nothing is written to the store and no rule is saved into a
shapes graph. `heuristic_rules` in a `POST …/repair` body runs rules of
your own under the same restrictions.

---

## Configuration

| Variable | Default | |
|---|---|---|
| `OTS_REPAIR_MAX_QUADS` | memory limit ÷ 8 ÷ 512 B (2 000 000 when the limit cannot be read) | The most quads one run may copy. |
| `OTS_REPAIR_CONCURRENCY` | 1 | Runs at once (each holds a copy in memory). |
| `OTS_REPAIR_PROPOSAL_TTL` | 30 | Days a kept proposal lives. |

## Limits

- Nothing SPARQL-based is repaired; no literal is ever invented; no choice
  is made without an opt-in policy.
- The copy holds whole graphs (`scope.focus` narrows the rules, not the
  copy), and is validated whole.
- A placeholder never merges into a blank node, and a blank-node triple is
  never deleted.
- Kept proposals are not backed up and computing a proposal is not
  audited (an apply is).
- The patch lock orders the two patch routes only. A SPARQL update or a
  Graph Store write that lands between an apply's check and its write is
  not detected.
