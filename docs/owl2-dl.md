# OWL 2 DL

> **Open Triplestore role names:** class definitions and class axioms = graph role **Model** (the T-Box); ABox content = graph role **Instances**.  Property definitions and relations (the R-Box) belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning.  The DL reasoner reasons over the TBox+RBox schema together — the role split concerns where terms are stored and registered, not the reasoning semantics.

OWL 2 DL (SROIQ(D)) is the most expressive OWL 2 profile. Reasoning in it needs a tableau-style reasoner. The `owl2-dl` regime runs on a **backend** you choose with `OTS_DL_BACKEND`. There is no default: unless a backend is configured, every `owl2-dl` request answers **503**.

| `OTS_DL_BACKEND` | What runs | Complete for OWL 2 DL? |
|---|---|---|
| *(unset)* | nothing — `owl2-dl` answers 503 | — |
| `native` | OWL 2 RL plus DL-syntax rules, in process | **no** — sound, not complete (`complete: false`) |
| `konclude` | the [Konclude](https://github.com/konclude/Konclude) binary (`OTS_KONCLUDE_BIN`) | yes, for what it reports (see below) |
| `sidecar` | an HTTP reasoner service (`OTS_REASONER_URL`) speaking the protocol below | as the sidecar says (`complete` in its answer) |

A failing external backend never falls back to the native rules: an unreachable backend is a 503, a run past the time limit a 504 with `"result": "unknown"`.

---

## The native backend

`Owl2DLReasoner` applies every OWL 2 RL/RDF rule ([owl2-rl.md](owl2-rl.md)) together with rules for DL syntax that forward chaining can still honour. It runs them to **one joint fixed point**: each round runs the RL rules to their own fixed point, then one pass of the DL rules, until a DL pass adds nothing. A DL consequence is therefore a premise for the RL rules, and the other way round. For example, `x loves x` from a self restriction reaches `rdfs:domain` and `rdfs:subPropertyOf`.

| Axiom | Rule | Behaviour |
|---|---|---|
| `owl:hasSelf true` on `p` | `dl-has-self` | every `x a C` gets `x p x` |
| `owl:hasSelf true` on `p` | `dl-has-self-converse` | every `x p x` gets `x a C` |
| `owl:ReflexiveProperty` | `dl-reflexive` | every individual in scope gets `x p x` (anything typed with a non-reserved class, `owl:Thing` or `owl:NamedIndividual`, and both ends of a `p` assertion), so `prp-irp` catches a property that is also irreflexive |
| `owl:disjointUnionOf` | `dl-disjoint-union-subclass` | each member is a subclass of the union |
| `owl:disjointUnionOf` | `dl-disjoint-union-pairwise` | members are pairwise `owl:disjointWith` |

Keys of any length and negative property assertions are handled by the RL rules (`prp-key`, `prp-npa1`, `prp-npa2`). Inconsistencies are reported with the RL rule that fired (`cax-dw`, `prp-irp`, `prp-npa1`, …).

**What the native backend cannot do.** It creates no existential witnesses, makes no case splits over unions and does no nominal reasoning. For example, from `A ⊑ ∃r.B`, `∃r.B ⊑ C` and `a : A` it does not derive `a : C`. Its reports therefore say `"complete": false` and carry a warning. Its checks answer `false` (that is sound) or `unknown`, never a `true` they cannot prove.

**Cardinality obligations.** Minimum and exact cardinalities (qualified or not) cannot be satisfied by forward chaining. The rules record each obligation as a `urn:dl:minCardinality`, `urn:dl:exactCardinality`, `urn:dl:minQualifiedCardinality` or `urn:dl:exactQualifiedCardinality` triple in a **diagnostics graph** next to the target: `<target>:diagnostics`, for example `urn:entailment:owl2-dl:diagnostics`. That graph is never folded into a query by `?entailment=`. Until this release these triples sat in the entailment graph itself.

```sparql
SELECT ?individual ?n
WHERE { GRAPH <urn:entailment:owl2-dl:diagnostics> { ?individual <urn:dl:minCardinality> ?n } }
```

---

## What every backend run checks first

1. **Size.** An external backend gets at most `OTS_REASONER_MAX_TRIPLES` triples (default 1,000,000). More is a **413** (`{triples, limit, backend}`).
2. **Identity policy.** `sameas-off` cannot be honoured by a DL reasoner, which always treats `owl:sameAs` as equality. With an external backend it is a **422**. The native backend honours it.
3. **OWL 2 DL profile.** The input is mapped from RDF to OWL 2: the reverse of the [OWL 2 RDF mapping](https://www.w3.org/TR/owl2-mapping-to-rdf/), covering declarations, all class expressions and data ranges, n-ary axioms, keys, chains, negative assertions, anonymous individuals and axiom/ontology annotations. The result is then checked against the typing constraints and global restrictions of OWL 2 DL. Input outside OWL 2 DL is a **422** `{in_profile: false, violations: [{rule, detail}]}`. The rules reported:

| `rule` | Meaning |
|---|---|
| `unmapped-triple` | a triple with no OWL 2 reading (for example `rdf:value`, `rdf:Statement`) |
| `property-punning` | one IRI used as two kinds of property (object/data/annotation) |
| `class-datatype-punning` | one IRI used as a class and a datatype |
| `reserved-vocabulary` | an `rdf:`/`rdfs:`/`owl:`/`xsd:` IRI declared or used as a class |
| `unknown-datatype` | a datatype outside the OWL 2 datatype map without a `DatatypeDefinition` (for example `xsd:date`) |
| `datatype-redefined`, `cyclic-datatype-definition`, `unknown-facet` | datatype definition errors |
| `top-data-property` | `owl:topDataProperty` anywhere but as a super data property |
| `non-simple-property` | a property with a transitive or chain-defined sub-property used in a cardinality or self restriction, or declared functional, inverse-functional, irreflexive, asymmetric or disjoint |
| `irregular-property-hierarchy` | property chains that define properties in terms of each other |
| `anonymous-individual` | anonymous individuals in SameIndividual/DifferentIndividuals/negative assertions/`ObjectOneOf`/`ObjectHasValue`, or linked in a cycle |

The mapping relaxes the spec in two places, as the OWL API does, and says so in `warnings`:
- **Typing by use.** An undeclared IRI gets its kind from how it is used. A predicate with a literal object is a data property, any other predicate an object property, an `rdf:type` object a class, and `rdfs:Class` reads as `owl:Class`.
- **Named restrictions.** An IRI carrying `owl:onProperty …` (or `owl:intersectionOf …`) reads as `EquivalentClasses(IRI expression)`.

`owl:imports` is never followed: a run reasons over exactly the graphs it was given. SWRL rules are not OWL 2 axioms. They are skipped with a warning, so a DL run never claims to have applied them.

---

## Konclude (`OTS_DL_BACKEND=konclude`)

[Konclude](https://github.com/konclude/Konclude) is LGPL-3.0 and is not shipped in the image. Install a release binary yourself and point `OTS_KONCLUDE_BIN` at it (default: `Konclude` on `PATH`). Konclude reads only OWL/XML or functional-style syntax, from files. A run therefore:

1. writes the mapped ontology as functional-style syntax to a scratch directory (under the system temp dir, removed afterwards);
2. runs `Konclude owllinkfile` on an OWLlink request with:
   - `IsKBSatisfiable` for consistency;
   - `GetSubClassHierarchy` for classification;
   - `GetFlattenedTypes` and `GetSameIndividuals` for each named individual;
3. runs `Konclude sparqlfile` with one `SELECT ?x ?y { ?x <p> ?y }` per object property, for the entailed object property assertions;
4. kills either process at `OTS_REASONER_TIMEOUT_SECS` (default 300).

What reaches the target graph — only triples about named entities, and only ones not already asserted:
- `rdfs:subClassOf` (the transitive closure, without `⊑ owl:Thing`) and `owl:equivalentClass` between named classes;
- `C rdfs:subClassOf owl:Nothing` for every unsatisfiable class;
- `rdf:type` of every named individual (without `owl:Thing`);
- `owl:sameAs` between named individuals;
- object property assertions between named individuals.

Konclude does not report data values entailed by `owl:hasValue` and ignores annotations, so **data property values are not materialised**. A run over data properties says so in `warnings`. An inconsistent ontology is a 422 with `rule: "external-reasoner"`, and nothing is written. Checked against Konclude v0.7.0-1138; the live tests (`OTS_TEST_KONCLUDE_BIN=/path/to/Konclude cargo test --test owl2_dl_conformance`) run a real binary.

**Data complements (`owl:datatypeComplementOf`).** Konclude v0.7.0-1138 says "consistent" for some inconsistent uses of a data complement:

| Input | Right answer | Konclude |
|---|---|---|
| `A ⊑ ∃p.¬rdfs:Literal`, `a : A` | inconsistent | consistent |
| `A ⊑ ∃p.(xsd:integer ⊓ ¬xsd:integer)`, `a : A` (also for `xsd:string`) | inconsistent | consistent |
| `p` has range `¬xsd:integer`, `p(a, 1)` | inconsistent | consistent |
| `p` has range `¬{1}`, `p(a, 1)` | inconsistent | consistent |

Every wrong answer seen so far is "consistent" where the right one is "inconsistent". Konclude never reported a clash that is not there. Other shapes come out right: `a : ∀p.¬{1}` with `p(a, 1)`, `a : ∀p.¬xsd:integer` with `p(a, 1)`, and range `xsd:integer` with `a : ∃p.¬xsd:integer` are all inconsistent, as they should be. The server therefore:

- writes `¬rdfs:Literal` as the empty data range, `¬¬D` as `D`, and the complement of an empty range as `rdfs:Literal`, so these never reach Konclude as complements;
- when the input still contains a data complement after that, gives no answer that depends on Konclude finding no clash. A check answers `unknown`, with a `detail` that says why, instead of `consistent: true`, `entailed: false` or `satisfiable: true`. A materialisation reports `complete: false` and a warning, and still writes what Konclude derived. Answers that rest on a clash are kept: inconsistent, entailed, unsatisfiable.

A complement that only appears in an entailment conclusion is treated the same way. The bridge's own negations (`∀p.¬{v}` for a data assertion, `∃p.¬R` for a range) are not affected: they come out right in the probes, once `¬rdfs:Literal` is written as the empty range. The test `dl_konclude_live_still_misses_the_complement_clash` fails once the Konclude in use reads `∃p.(D ⊓ ¬D)` correctly, and that is the time to narrow this fallback.

---

## Reasoner sidecar (`OTS_DL_BACKEND=sidecar`)

A sidecar is an HTTP service that wraps a DL reasoner. Configure it with:
- `OTS_REASONER_URL` (base URL);
- `OTS_REASONER_TOKEN` (sent as `Authorization: Bearer …`);
- `OTS_REASONER_TIMEOUT_SECS` and `OTS_REASONER_MAX_TRIPLES`.

The client waits a quarter longer than the timeout (at most 5 s more), so the sidecar's own answer can arrive first. Protocol, version 1:

**`POST /v1/reason`** — body `{"data": "<N-Triples>", "timeout_ms": n}`; answer:

```json
{
  "consistent": true,
  "inconsistency": null,
  "in_profile": true,
  "violations": [],
  "unsatisfiable": ["http://example.org/Impossible"],
  "inferred": "<http://example.org/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/B> .\n",
  "complete": true,
  "backend": { "name": "hermit", "version": "1.4.5" },
  "warnings": []
}
```

**`POST /v1/check`** — body `{"task": "consistency" | "entailment" | "satisfiability", "data", "conclusion"?, "class"?, "timeout_ms"}`; answer `{"result": "true" | "false" | "unknown", "detail"?, "in_profile", "violations", "backend", "warnings"}`.

How the server reads the sidecar's answers:
- a sidecar that cannot be reached, or answers 503, makes the request a 503;
- a 504, or no answer in time, makes it a 504 with result unknown;
- `in_profile: false`, or a 422 with `violations`, makes it a 422 with those violations;
- any other non-200 is a 502;
- inferred triples with a blank node, and triples that were already asserted, are dropped.

---

## Endpoints

### Materialise — `POST /api/reasoning/materialize`

`{"regime": "owl2-dl", "dataset"?, "source_graphs"?, "target_graph"?}`. The answer carries:
- `backend` (`native`, `konclude` or `sidecar`), `backend_version`, `complete` and `warnings`;
- `consistent: true` when the run found no inconsistency.

An inconsistent input is the same 422 as for `owl2-rl`: `{consistent: false, rule, detail, regime, target_graph}`. Add **`?async=true`** to queue the run instead: the answer is a 202 with `{job_id, status, location}`.

### Check — `POST /api/reasoning/check`

OWL 2 Conformance §2.2 checks. The body is `{"task": …}`, plus where the premise comes from and what the task needs:

| `task` | Needs | `result` |
|---|---|---|
| `consistency` | — | `true` / `false` / `unknown` |
| `entailment` | `conclusion` (Turtle) | does the premise entail every axiom of the conclusion? |
| `satisfiability` | `class` (IRI) | can the class have an instance? |
| `profile` | — | is the premise in OWL 2 DL? (`in_profile`, `violations`) — needs **no** backend |

The premise is one of:
- a dataset's conformance layer (`dataset`), limited to the graphs the caller can read;
- `source_graphs` the caller can read;
- Turtle in `premise`;
- if none is given, the unnamed default graph.

A check that ran answers **200** with `{task, result, backend, backend_version, complete, regime, sources, warnings}`. An inconsistent premise adds the materialisation 422's fields `consistent: false`, `rule`, `detail`. Errors use the same statuses as materialisation: 422 (not in OWL 2 DL), 503, 504 with `"result": "unknown"`, 413 and 502.

How each backend answers:
- **Konclude** reduces entailment to consistency (`O ⊨ α` iff `O ∪ ¬α` is inconsistent), one knowledge base per negated axiom. Axioms with no such reduction answer `unknown`: data sub-properties, keys, datatype definitions, conclusions with anonymous individuals. A premise or conclusion with `owl:datatypeComplementOf` answers `unknown` where Konclude found no clash; see "Data complements" under [Konclude](#konclude-ots_dl_backendkonclude).
- **Native** answers entailment `true` when the conclusion (blank nodes as variables) matches the premise plus its closure, and `unknown` otherwise.

### Jobs — `GET /api/reasoning/jobs/{job_id}`

Returns `{id, kind, status: queued|running|succeeded|failed, created_at, started_at, finished_at, http_status, result}`. `http_status` and `result` are what the synchronous call would have answered, so a job that met an inconsistency holds the 422 body. Jobs are kept in memory for an hour after they finish, and a restart forgets them. Only the user who started a job, or an admin, can see it.

### Per-dataset runs

`PUT /api/datasets/{id}/entailment {"regime": "owl2-dl", "mode": "materialize"}` runs at once, like every regime. After a write to the dataset's graphs, an `owl2-dl` dataset is **not** re-materialised inside the write, unlike the other regimes. Instead:
- a background run starts once no write has arrived for `OTS_DL_DEBOUNCE_MS` (default 2000);
- writes that arrive during a run queue one more run.

`GET /api/datasets/{id}/entailment` reports:
- `status`: `queued` and `running` while a run is pending, then `ok`, `inconsistent`, `not_in_profile`, `unavailable`, `timeout`, `too_large` or `failed`;
- `error`, `backend` and `complete`;
- `dl_backend`, the server's configured backend.

The entailment graph is eventually consistent with the data.

### Querying

`?entailment=owl2-dl` folds `urn:entailment:owl2-dl` into a query; `?entailment_dataset=<id>` folds the dataset's `urn:entailment:owl2-dl:<id>`. This is the store's own regime selector, not the SPARQL 1.1 Entailment Regimes specification.

---

## Rust API

```rust
use open_triplestore::reasoning::owl2_dl::Owl2DLReasoner;

let report = Owl2DLReasoner::new(&store)
    .with_target("urn:entailment:owl2-dl")   // optional — this is the default
    .materialize()?;                          // the native joint fixed point
```

`reasoning::dl_backend::{materialize, check}` run a configured backend
(`reasoning::dl_config::DlConfig`), and `reasoning::dl_backend::DlBackend` is the trait an external reasoner implements.

---

## References

- [W3C OWL 2 Structural Specification](https://www.w3.org/TR/owl2-syntax/) (§5.8.1 typing constraints, §11 global restrictions)
- [W3C OWL 2 Mapping to RDF Graphs](https://www.w3.org/TR/owl2-mapping-to-rdf/)
- [W3C OWL 2 Profiles](https://www.w3.org/TR/owl2-profiles/)
- [W3C OWL 2 Direct Semantics](https://www.w3.org/TR/owl2-direct-semantics/)
- [OWL 2 Conformance](https://www.w3.org/TR/owl2-conformance/)
- [OWLlink](https://www.w3.org/Submission/owllink-structural-specification/)
