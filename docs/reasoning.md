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
— instance, model, vocabulary, domain-value and linkset graphs — into the
dataset's own entailment graph `urn:entailment:<regime>:<id>`. The graph is
rebuilt, not appended to, so consequences of deleted data disappear, and no
two datasets share inferred triples. `mode: off` clears it.

Queries opt in per request:

```
GET /sparql?query=…&entailment_dataset=<id>            # the configured regime
GET /sparql?query=…&entailment_dataset=<id>&entailment=owl2-rl
POST /sparql  (application/sparql-query with the same query parameters,
               or application/x-www-form-urlencoded fields)
```

`GET /api/datasets/<id>/entailment` reports the regime, mode, graph and the
last run. The global `?entailment=<regime>` (the shared
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
