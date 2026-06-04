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
{ "regime": "rdfs|owl2-rl|owl2-el|owl2-ql|owl2-dl", "target_graph": "<optional IRI>" }
```

The response is a count of the inferred triples added. Query the current status of all entailment graphs via `GET /api/reasoning/status`.

For OWL 2 QL you can rewrite a query against the schema instead of materialising — `POST /api/reasoning/rewrite` returns the expanded SPARQL. You can also fold an entailment graph into a single query by adding `?entailment=rdfs|owl2-rl|owl2-el|owl2-ql` to a SPARQL request.

## SWRL rules

Beyond the standard profiles, SWRL (Semantic Web Rule Language) Horn-clause rules derive new triples from custom *antecedent → consequent* patterns — useful for domain logic that doesn't fit an OWL profile. Submit rules to `POST /api/swrl/execute`.
