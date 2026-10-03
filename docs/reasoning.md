# OWL Reasoning

Reasoning can be applied to materialise inferred triples across all named graphs. Inferred triples are written to dedicated entailment graphs and can be queried or cleared independently. The entailment graph IRIs are: `urn:entailment:rdfs`, `urn:entailment:owl2-rl`, `urn:entailment:owl2-el`, `urn:entailment:owl2-ql`, `urn:entailment:owl2-dl`.

| Profile | Best for | Notes |
|---|---|---|
| RDFS | Simple schema inference | Lowest overhead. Infers subclass hierarchies, property domains and ranges. |
| OWL 2 QL | Large read-heavy datasets | No existentials. Uses query rewriting — minimal extra storage. |
| OWL 2 EL | Life sciences (SNOMED-CT, Gene Ontology) | Supports existential restrictions. Polynomial time. |
| OWL 2 RL | Rule-based integration with RDF | Materialises triples. Most complete; may significantly grow graph size. |
| OWL 2 DL | Full OWL expressivity | Native support for `hasSelf`, `disjointUnionOf`, `NegativePropertyAssertion`, `hasKey` (1–2 keys), and cardinality annotations on top of all OWL 2 RL rules. Full existential completion (tableau) requires an external reasoner (HermiT, Pellet). |

Reasoning is triggered via `POST /api/reasoning/materialize` with a JSON body:

```json
{
  "regime": "rdfs|owl2-rl|owl2-el|owl2-ql|owl2-dl",
  "target_graph": "<optional IRI>",
  "dataset": "<optional dataset id>",
  "source_graphs": ["<optional graph IRIs>"]
}
```

**What the rules read.** With `dataset`, the reasoner works on that dataset's
*conformance layer* — its data-bearing graphs (instances, model, vocabulary,
domain values, linksets, unclassified) plus the graphs of the model version it
declares conformance to — and nothing else; `GET /api/datasets/:id/conformance`
shows exactly that set. With `source_graphs`, the listed graphs (each must be
readable by the caller); with `dataset` *and* `source_graphs`, both. With
neither, the rules read the unnamed default graph only, as they historically
did — which means a dataset's named graphs are invisible to an unscoped run,
so pass `dataset` for anything loaded through the dataset APIs. The scope is
applied at the store level (a `USING` dataset on every rule), so all regimes
behave the same.

The response is a count of the inferred triples added. Query the current status of all entailment graphs via `GET /api/reasoning/status`.

For OWL 2 QL you can rewrite a query against the schema instead of materialising — `POST /api/reasoning/rewrite` returns the expanded SPARQL. You can also fold an entailment graph into a single query by adding `?entailment=rdfs|owl2-rl|owl2-el|owl2-ql|owl2-dl` to a SPARQL request.

## Per-dataset entailment: selectable regime, materialisation toggle

A dataset can select its own regime and keep it materialised:

```bash
curl -X PUT http://localhost:7878/api/datasets/<id>/entailment \
  -H "Authorization: Bearer <token>" -H 'Content-Type: application/json' \
  -d '{"regime": "rdfs", "mode": "materialize"}'
```

In `materialize` mode the regime runs at once and again after every write
to one of the dataset's graphs (Graph Store, SPARQL Update, imports, restores,
patches, LDES syncs, property states), over the dataset's conformance layer
— instance, model, vocabulary and domain-value graphs, plus linkset graphs
when the [identity policy](#identity-policy--what-happens-with-owlsameas)
is `sameas-full` — into the dataset's own entailment graph
`urn:entailment:<regime>:<id>`. A write that only **adds** quads (a Graph
Store `POST`) *extends* the graph: the rules are monotone, so they re-run to
their fixed point on top of the existing consequences and nothing is cleared.
Every other write (a `PUT`, a `DELETE`, a SPARQL Update, a restore) rebuilds
the graph from scratch, so consequences of deleted data disappear. A write
that left the store's write generation unchanged since the last run triggers
nothing. No two datasets share inferred triples; `mode: off` clears the graph.

Queries opt in per request:

```
GET /sparql?query=…&entailment_dataset=<id>            # the configured regime, else the stored rules' graph
GET /sparql?query=…&entailment_dataset=<id>&entailment=owl2-rl
POST /sparql  (application/sparql-query with the same query parameters,
               or application/x-www-form-urlencoded fields)
```

`GET /api/datasets/<id>/entailment` reports the regime, mode, graph and the
last run, and the dataset's SWRL rules: rules stored with a dataset always
run with it, joining the regime's fixed point (see
[Rules stored with a dataset](#rules-stored-with-a-dataset)). The global `?entailment=<regime>` (the shared
`urn:entailment:<regime>` graphs filled by `POST /api/reasoning/materialize`)
keeps working unchanged.

## Identity policy — what happens with `owl:sameAs`

`owl:sameAs` says two IRIs name **one** thing, and the OWL 2 RL equality
rules take that literally: every property of the one becomes a property of the
other (`eq-rep-*`), in both directions and transitively. That is exactly right
when two IRIs were minted for the same record. It is wrong for almost every
link between sources — a design-stage model element is not the as-built asset,
a registration record is not the physical object, a footprint on the map is
not the thing it outlines — and once such a link is materialised, attributes,
geometries and lifecycle states leak between the two representations and every
"trusted source" claim collapses (Beck, Abualdenien & Borrmann, LDAC 2021).

The **identity policy** controls this per dataset. It has three settings:

| Setting | Equality rules (`eq-sym`, `eq-trans`, `eq-rep-*`) | Linkset graphs as premises | Use when |
|---|---|---|---|
| `sameas-off` | do not run | no | the dataset should never merge two IRIs, whatever the data says |
| `sameas-narrow` — **built-in default** | run over the dataset's own graphs | no | two IRIs inside *one* registration may mean one record; links to other sources are correspondences, not identity |
| `sameas-full` | run | yes | you really want every `owl:sameAs`, linksets included, to merge (the behaviour before the policy existed) |

Whatever the setting, a **typed correspondence** is never treated as identity:
`prov:specializationOf` (a model object specialises the asset),
`prov:alternateOf` (two representations of one thing in different contexts),
`skos:exactMatch` / `closeMatch` / `broadMatch` / `narrowMatch` / `relatedMatch`
and `rdfs:seeAlso` never feed the equality rules. Use them — in a
`linkset`-role graph — for links between sources; keep `owl:sameAs` for
co-reference within one source.

**Example.** An asset register has `ex:bridge-17` with `ex:status "in
service"`; a BIM delivery, in the same dataset's linkset graph, says
`bim:elem-4711 owl:sameAs ex:bridge-17`, and the model element carries
`ifc:GlobalId "2F…"` and a design geometry.

- `sameas-full`: after materialisation `ex:bridge-17` has an `ifc:GlobalId`
  and a design geometry, and `bim:elem-4711` is "in service" — the model
  element inherited the asset's lifecycle state and vice versa.
- `sameas-narrow` (default): the linkset is not a premise; nothing crosses.
  Change the link to `bim:elem-4711 prov:specializationOf ex:bridge-17` and it
  stays a queryable, typed correspondence with no propagation at all.
- `sameas-off`: as narrow, and even an `owl:sameAs` between two IRIs inside
  the register's own instance graph stays plain data.

**Where it is set.** Per organisation (every dataset the organisation owns
inherits it) and per dataset (overrides the organisation's setting); a dataset
with neither uses the built-in default. Everything an organisation or dataset
administrator needs is in the settings endpoints:

```bash
# What applies to a dataset, and why (setting on the dataset, its organisation, or the default)
curl http://localhost:7878/api/datasets/<id>/identity -H "Authorization: Bearer <token>"
# → {"policy":"sameas-narrow","source":"organisation","setting":null,"options":[…]}

# Set it for one dataset (re-materialises at once when the dataset is in `materialize` mode)
curl -X PUT http://localhost:7878/api/datasets/<id>/identity   -H "Authorization: Bearer <token>" -H 'Content-Type: application/json'   -d '{"policy": "sameas-full"}'
# Remove the dataset's own setting again (falls back to the organisation / default)
curl -X DELETE http://localhost:7878/api/datasets/<id>/identity -H "Authorization: Bearer <token>"

# Set it for every dataset an organisation owns (organisation admins)
curl -X PUT http://localhost:7878/api/organisations/<org-id>/identity   -H "Authorization: Bearer <token>" -H 'Content-Type: application/json'   -d '{"policy": "sameas-off"}'
```

`GET /api/datasets/<id>/entailment` reports the effective policy too
(`identity`, `identity_source`), and `PUT …/entailment` accepts an optional
`"identity"` next to `regime`/`mode` (`"inherit"` removes the dataset's own
setting). The policy applies to `POST /api/reasoning/materialize` with a
`dataset` as well; an unscoped run (no dataset) keeps `sameas-full`, as it
reads whatever graphs it is given.

## SWRL rules

Beyond the standard profiles, SWRL (Semantic Web Rule Language) Horn-clause rules derive new triples from custom *antecedent → consequent* patterns — useful for domain logic that doesn't fit an OWL profile. Submit rules to `POST /api/swrl/execute`.

Rules come in six syntaxes, chosen with `format`:

| `format` | Syntax | Notes |
|---|---|---|
| `text` (default) | `http://ex/A(?x) ^ http://ex/p(?x, ?y) -> http://ex/B(?y)` | Absolute IRIs only, no built-ins. |
| `xml` (or `owlxml`) | OWL/XML `DLSafeRule`, as the OWL API and Protégé write it | `Prefix` declarations, `abbreviatedIRI`, `xml:base`, every `Literal` form. |
| `rdf` | The SWRL RDF syntax (`swrl:Imp`, `swrl:body`/`swrl:head` lists) | Any RDF serialisation: `rdf_format` is `turtle` (default), `ntriples`, `nquads`, `trig`, `rdfxml`, `jsonld`, `n3` or a media type. `base_iri` resolves relative IRIs. |
| `functional` | OWL 2 functional-syntax `DLSafeRule`, inside an `Ontology(…)` or on its own | `Prefix(…)` declarations; other axioms are skipped. |
| `swrlapi` | The SWRLAPI human-readable syntax: `ex:Person(?p) ^ swrlb:greaterThan(?age, 17) -> ex:Adult(?p)` | Prefixes from the request's `prefixes` (`""` is the default prefix, for bare names), then the server's prefix registry. |
| `ruleml` | The SWRL §4 XML concrete syntax (`ruleml:imp`, `swrlx:*Atom`) | DTD entities such as `&swrlb;` are not expanded: write full IRIs. |

The text form and the SWRLAPI syntax have no property declarations, so a property atom whose second argument is a variable matches individuals and literals alike. A literal there makes it a data property atom, and an individual an object property atom.

Rule bodies read the unnamed default graph, unless the request names a `dataset`, `source_graphs`, or both. Then they read those graphs, with the read checks of `/api/reasoning/materialize`: only the dataset graphs the caller may read, and every explicit graph must be readable. Derived triples go to `target_graph`, an absolute IRI the caller may write. Without one, a dataset run writes to the dataset's inference graph, which only its writers may fill, and any other run writes to the default graph. That inference graph is rebuilt after writes to the dataset, so a one-off run's results last until the next write; to keep a rule, store it with the dataset (below).

Every rule is checked before any runs, and one the server cannot run as written refuses the whole request with `400`, so nothing is written:

- an element or atom the reader does not understand: class-expression atoms, `DataRangeAtom` and anonymous individuals are not supported yet;
- an unsafe rule: a head variable the body does not bind, or a built-in variable only a built-in mentions (built-ins check values but cannot bind them);
- a built-in in the head, or one outside the supported `swrlb:` comparisons, arithmetic, `stringConcat`, `contains` and `matches`;
- a literal where an individual belongs, or the other way round.

The response reports `iterations`, `triples_inferred` (what this run wrote to the target graph), `converged` with its `stop_reason` (`fixpoint`, `max_iterations` or `timeout`), `target_graph`, and `sources`, the graphs rule bodies read (`null` for the default graph). Execution counts against the server's limit on concurrent expensive operations and stops at the write timeout.

### Rules stored with a dataset

`swrl:Imp` rules in the SWRL RDF syntax that a dataset keeps always run. They are read from the dataset's `entailment`- and `model`-role graphs and from the graphs of the model version it conforms to. They read the same reasoning sources as the dataset's regime, and they re-run after every write to one of the dataset's graphs, the rule graphs included:

- **The dataset has a regime in `materialize` mode.** The rules and the regime run to one joint fixed point in the regime's graph (`urn:entailment:<regime>:<dataset>`). Each sees what the other derives.
- **It has none.** The rules run on their own into `urn:entailment:swrl:<dataset>`.

Either graph is the dataset's inference graph, which `?entailment_dataset=<id>` adds to a query. `GET /api/datasets/{id}/entailment` reports it as `inference_graph`, along with `rules`: the graphs read, the number of rules (or why they cannot be read), and the last run (`rounds`, `triples_inferred`, `converged`). The rule-reading needs the `swrl` feature.
