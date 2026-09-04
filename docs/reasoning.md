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

## SWRL rules

Beyond the standard profiles, SWRL (Semantic Web Rule Language) Horn-clause rules derive new triples from custom *antecedent → consequent* patterns — useful for domain logic that doesn't fit an OWL profile. Submit rules to `POST /api/swrl/execute`.
