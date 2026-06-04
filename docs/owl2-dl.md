# OWL 2 DL

> **Open Triplestore role names:** TBox content = graph role **Model**; ABox content = graph role **Instances**.

OWL 2 DL (Description Logics, based on SROIQ(D)) is the most expressive OWL 2 profile and the basis for fully formal ontology engineering.  It is N2EXPTIME-complete, which means full reasoning requires a tableau algorithm with blocked-node merging.  This triplestore provides **native support** for all OWL 2 RL rules plus the DL-specific axioms that are expressible as SPARQL INSERT operations, and an **external reasoner bridge** for full tableau completion.

---

## What is supported natively

The native `Owl2DLReasoner` runs in two phases:

### Phase 1 — All OWL 2 RL rules (~80 rules)

All OWL 2 RL forward-chaining rules from W3C OWL 2 Profiles Tables 4–9 are applied first.  These cover the large majority of practical ontology inferences, including:

| Rule group | Examples |
|---|---|
| Class axioms (`cls-*`) | subClassOf, equivalentClass, disjointWith, unionOf, intersectionOf |
| Property axioms (`prp-*`) | subPropertyOf, equivalentProperty, inverseOf, domain, range, functionalProperty, transitiveProperty, symmetricProperty, propertyChain |
| Schema entailment (`scm-*`) | Schema-level subClassOf, equivalentClass, subPropertyOf transitivity |
| Equality (`eq-*`) | sameAs propagation and symmetry |
| Inconsistency detection | owl:Nothing membership, disjointWith violations, functional property conflicts |

### Phase 2 — DL extension rules

The following DL-specific axioms are added on top of the RL rules:

| Axiom | Rule | Behaviour |
|---|---|---|
| `owl:hasSelf` | `dl-has-self` | If C has a self-restriction on p, any x of type C gets `x p x` |
| `owl:disjointUnionOf` | `dl-disjoint-union-subclass` | Each list member Ci gets `Ci rdfs:subClassOf C` |
| `owl:disjointUnionOf` | `dl-disjoint-union-pairwise` | All list members are pairwise `owl:disjointWith` |
| `owl:NegativePropertyAssertion` | consistency check | Raises `Inconsistency` error if a declared NPA triple actually exists |
| `owl:hasKey` (1 property) | `dl-has-key-one` | Two individuals of type C with the same key value → `owl:sameAs` |
| `owl:hasKey` (2 properties) | `dl-has-key-two` | Two individuals sharing both key values → `owl:sameAs` |
| `owl:minCardinality` | annotation | Records `urn:dl:minCardinality` on qualifying individuals |
| `owl:cardinality` | annotation | Records `urn:dl:exactCardinality`; max side handled by RL `cls-maxc2` |
| `owl:minQualifiedCardinality` | annotation | Records `urn:dl:minQualifiedCardinality` |
| `owl:qualifiedCardinality` | annotation | Records `urn:dl:exactQualifiedCardinality` |

---

## What requires an external tableau reasoner

The following features require a tableau algorithm with blocked-node merging and cannot be expressed as SPARQL INSERT rules:

- **Existential witness generation** — when `owl:minCardinality n ≥ 1` applies but no n filler nodes exist, a tableau creates fresh anonymous individuals (Skolem witnesses).  SPARQL cannot create new nodes in an INSERT.
- **Full ABox completion** — propagating universal quantifiers (`owl:allValuesFrom`) across cyclic role paths requires cycle detection via blocking.
- **Nominals** (`owl:oneOf`) combined with complex role hierarchies.
- **`owl:hasKey` with more than 2 properties** — the native rules only handle 1- and 2-property key lists.

To get full OWL 2 DL reasoning, plug in an external reasoner (see below).

> **Without Konclude:** When no external reasoner is configured, the `NativeTableauStub`
> in `src/reasoning/owl2_dl.rs` is used as a fallback.  It satisfies the `ExternalReasoner`
> trait interface and runs the RL+DL-extension rules described above, returning partial results.
> This provides OWL 2 RL-level coverage without a full tableau.  All Phase 1 + Phase 2 rules
> above will fire; only the four tableau-only features listed here will be absent.
>
> **Important — what the stub does *not* do:** the stub only supports *rule
> materialization* (`materialize()` / `?entailment=owl2-dl`). The dedicated
> description-logic services — **classification**, **consistency checking**, and
> **explicit inference extraction** (`classify()`, `check_consistency()`,
> `get_inferences()`) — return a `NotSupported` error unless a real external
> reasoner (HermiT, Pellet, ELK, Konclude, …) is plugged in. If your workflow
> needs sound-and-complete DL classification or consistency, you **must** configure
> an external reasoner; the bundled engine alone is not a complete OWL 2 DL reasoner.

---

## Known limitations

| Feature | Limitation |
|---|---|
| `owl:hasKey` | Only key lists of **1 or 2 properties** are handled natively.  Longer lists silently produce no `sameAs` for those combinations. |
| `owl:minCardinality` | Only an annotation triple (`urn:dl:minCardinality`) is inserted to record the obligation.  Existential fillers are NOT generated. |
| `owl:cardinality` | The min side is annotated only; the max side (cls-maxc1/cls-maxc2) is handled by RL. |
| Qualified cardinalities | Same annotation-only behaviour as unqualified cardinalities. |

---

## Usage

### Rust API

```rust
use open_triplestore::reasoning::owl2_dl::Owl2DLReasoner;

let report = Owl2DLReasoner::new(&store)
    .with_target("urn:entailment:owl2-dl")   // optional — this is the default
    .materialize()?;

println!("Regime: {}", report.regime);          // "owl2-dl"
println!("Triples added: {}", report.triples_added);
println!("Elapsed: {}ms", report.elapsed_ms);
```

### SPARQL endpoint

Pass `?entailment=owl2-dl` to any query to include the entailed triples:

```
GET /sparql?query=SELECT+...&entailment=owl2-dl
```

The entailed triples live in the named graph `urn:entailment:owl2-dl` and are automatically included when that entailment regime is requested.

### Querying the entailment graph directly

```sparql
SELECT ?s ?p ?o
WHERE {
  GRAPH <urn:entailment:owl2-dl> { ?s ?p ?o }
}
LIMIT 100
```

### Checking cardinality obligations

```sparql
SELECT ?individual ?n
WHERE {
  GRAPH <urn:entailment:owl2-dl> {
    ?individual <urn:dl:minCardinality> ?n
  }
}
```

---

## Connecting an external reasoner

### Konclude (built-in bridge)

[Konclude](https://www.derivo.de/en/products/konclude/) is a high-performance OWL 2 DL reasoner
written in C++ and available under the Apache 2.0 licence.  A ready-to-use bridge is included:

```rust
use open_triplestore::reasoning::konclude_bridge::KoncludeReasoner;
use open_triplestore::reasoning::owl2_dl::ExternalReasonerBridge;

// Finds "Konclude" in PATH
let konclude = KoncludeReasoner::new();

// Or specify the binary path explicitly
let konclude = KoncludeReasoner::new().with_binary("/opt/konclude/Konclude");

let bridge = ExternalReasonerBridge::new(Box::new(konclude));
let report  = bridge.materialize(&store, &[], "urn:entailment:owl2-dl")?;
println!("OWL 2 DL: {} triples", report.triples_added);
```

**Installing Konclude:**

```bash
# macOS (Homebrew tap or direct download)
curl -L https://github.com/konclude/Konclude/releases/latest/download/Konclude-linux-x86_64 \
  -o /usr/local/bin/Konclude && chmod +x /usr/local/bin/Konclude

# Verify
Konclude --version
```

The bridge:
1. Runs all native OWL 2 DL rules in-process.
2. Serialises the store to Turtle.
3. Passes it to Konclude via stdin (`Konclude realization -i - -o -`).
4. Parses the class hierarchy from the response and loads it into the target graph.

If Konclude is not in PATH, step 3 is skipped and only native results are returned
(no error — the native rules still provide substantial coverage).

### Custom external reasoner

Implement the `ExternalReasoner` trait for any OWL DL reasoner and pass it to
`ExternalReasonerBridge`:

```rust
use open_triplestore::reasoning::owl2_dl::{ExternalReasoner, ExternalReasonerBridge, ReasoningError};

struct HermitBridge;

impl ExternalReasoner for HermitBridge {
    fn name(&self) -> &'static str { "hermit" }

    fn classify(&self, ontology_turtle: &str) -> Result<String, ReasoningError> {
        // Call HermiT via subprocess or HTTP, return Turtle subsumption hierarchy
        todo!()
    }

    fn check_consistency(&self, ontology_turtle: &str) -> Result<bool, ReasoningError> {
        todo!()
    }

    fn get_inferences(&self, ontology_turtle: &str) -> Result<String, ReasoningError> {
        // Return all inferred triples as Turtle
        todo!()
    }
}

let bridge = ExternalReasonerBridge::new(Box::new(HermitBridge));
let report = bridge.materialize(&store, &[], "urn:entailment:owl2-dl")?;
```

The bridge first runs all native DL rules, then calls `get_inferences()` on the external
reasoner and loads the result into the target graph.  If only the `NativeTableauStub` is
connected (the default), the bridge still succeeds and returns the partial native results.

---

## Entailment graph

All inferred triples — from both RL rules and DL extension rules — are written to the named graph `urn:entailment:owl2-dl` (configurable via `with_target()`).  The default graph is never modified.

---

## Comparison with other OWL 2 profiles

| Profile | Basis | Extra features vs RL |
|---|---|---|
| OWL 2 RL | ~80 forward-chaining rules | — |
| **OWL 2 DL** (this) | All RL rules + 10 DL extension rules | hasSelf, disjointUnionOf, NPA checks, hasKey (1-2 keys), cardinality annotations |
| OWL 2 DL (full tableau) | External reasoner required | Full existential completion, nominals+roles, hasKey n>2 |

---

## References

- [W3C OWL 2 Profiles](https://www.w3.org/TR/owl2-profiles/)
- [W3C OWL 2 Direct Semantics](https://www.w3.org/TR/owl2-direct-semantics/)
- [OWL 2 Conformance](https://www.w3.org/TR/owl2-conformance/)
