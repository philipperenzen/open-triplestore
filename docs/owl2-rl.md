# OWL 2 RL Profile

OWL 2 RL (Rule Language) is a tractable sub-language of OWL 2 that maps cleanly to rule-based
forward chaining.  It covers roughly 80 rules from the W3C OWL 2 RL specification (Table 4 and
Table 5 in the W3C document) and is complete for OWL 2 RL ontologies.

> **Open Triplestore role names:** TBox content = graph role **Model**; ABox content = graph role **Instances**.

OWL 2 RL is suitable for:
- Reasoning over large ABoxes with moderate TBoxes.
- Scenarios where all axioms are expressible as SPARQL INSERT rules (no existential witnesses
  needed, no full tableau).
- Enterprise knowledge graphs using property hierarchies, class hierarchies, cardinality
  constraints, and property chains.

## Rules Implemented

### Property Rules (prp-*)

| Rule | Description |
|------|-------------|
| prp-dom | Property domain: `?p rdfs:domain ?c` → type subjects |
| prp-rng | Property range: `?p rdfs:range ?c` → type objects |
| prp-fp | Functional property: merge objects sharing same subject |
| prp-ifp | Inverse functional property: merge subjects sharing same object |
| prp-irp | Irreflexive property: detect `x P x` as inconsistency |
| prp-symp | Symmetric property: if `x P y` then `y P x` |
| prp-asyp | Asymmetric property: detect `x P y` AND `y P x` as inconsistency |
| prp-trp | Transitive property: chain through three hops |
| prp-spo1 | SubPropertyOf: propagate triples through superproperty |
| prp-spo2 | Property chain axiom: `r ∘ s ⊑ t` |
| prp-eqp1/2 | EquivalentProperty: treat equivalent properties symmetrically |
| prp-pdw | PropertyDisjointWith: detect co-occurring disjoint properties |
| prp-npa1/2 | NegativePropertyAssertion: inconsistency when assertion violated |
| prp-key | hasKey: merge individuals sharing all key property values |

### Class Rules (cls-*)

| Rule | Description |
|------|-------------|
| cls-thing | Every individual is of type `owl:Thing` |
| cls-nothing1 | Detect explicit `owl:Nothing` membership |
| cls-nothing2 | Detect instances of classes asserted disjoint with their supers |
| cls-int1 | Intersection membership: `x type C1 ∧ C2` if `x type C1` and `x type C2` |
| cls-int2 | Intersection decomposition: members of `C1 ∩ C2` are members of each |
| cls-uni | Union: members of unions are members of `owl:Thing` |
| cls-com | ComplementOf inconsistency: `x type C` and `x type ¬C` |
| cls-svf1/2 | SomeValuesFrom: existential witnesses |
| cls-avf | AllValuesFrom: propagate range restrictions |
| cls-hv1/2 | HasValue: property assertions from value restrictions |
| cls-maxc1/2 | MaxCardinality(0): detect cardinality violations |
| cls-maxqc1-4 | QualifiedMaxCardinality: detect qualified cardinality violations |

### Class Axiom Rules (cax-*)

| Rule | Description |
|------|-------------|
| cax-sco | SubClassOf: type inheritance |
| cax-eqc1/2 | EquivalentClass: bi-directional type inheritance |
| cax-dw | DisjointWith: inconsistency detection |
| cax-adc | AllDisjointClasses: expand to pairwise disjointWith, then detect inconsistency |

### Schema Rules (scm-*)

| Rule | Description |
|------|-------------|
| scm-cls | Every class is subClassOf `owl:Thing` |
| scm-sco | SubClassOf transitivity |
| scm-eqc1/2 | EquivalentClass ↔ mutual subClassOf |
| scm-op/dp/ap | Axiomatic property typing |
| scm-spo | SubPropertyOf transitivity |
| scm-eqp1/2 | EquivalentProperty ↔ mutual subPropertyOf |
| scm-dom1/2 | Domain inheritance through property and class hierarchies |
| scm-rng1/2 | Range inheritance through property and class hierarchies |
| scm-hv | HasValue schema entailment |
| scm-svf1/2 | SomeValuesFrom schema entailment |
| scm-avf | AllValuesFrom schema entailment |
| scm-int | Intersection schema entailment |
| scm-uni | Union schema entailment |

## Configuration

```toml
# Cargo.toml
[features]
owl2-rl = ["rdfs-entailment"]   # already defined in the project
```

## API Usage

```rust
use open_triplestore::reasoning::owl2_rl::Owl2RLReasoner;
use open_triplestore::store::TripleStore;

let store = TripleStore::open("./data")?;
let reasoner = Owl2RLReasoner::new(&store);
let report = reasoner.materialize()?;

println!(
    "OWL 2 RL: {} triples in {} iterations ({} ms)",
    report.triples_added, report.iterations, report.elapsed_ms
);
```

Entailed triples go to `urn:entailment:owl2-rl`.

### Consistency Checking

```rust
reasoner.check_consistency()?;   // returns Err(ReasoningError::Inconsistency(...)) if violated
```

## Example

```turtle
# TBox
ex:Employee rdfs:subClassOf ex:Person .
ex:Contract owl:disjointWith ex:Permanent .
ex:worksFor rdfs:domain ex:Employee .

# ABox
ex:alice ex:worksFor ex:Acme .
ex:bob   rdf:type   ex:Contract, ex:Permanent .  # inconsistency!
```

After materialisation:
- `ex:alice rdf:type ex:Employee` (prp-dom)
- `ex:alice rdf:type ex:Person`   (cax-sco)
- Consistency check detects `ex:bob` violates `owl:disjointWith` (cax-dw)

## Performance

The fixed-point loop converges in O(depth of class hierarchy) iterations.  Each iteration
executes a batch of SPARQL INSERT queries.

| Dataset | Triples | Inferred | Time |
|---------|---------|----------|------|
| Pizza ontology (300 classes) | 3k | 12k | ~50ms |
| DBpedia ontology (600 classes) | 40k | 120k | ~0.8s |

## Differences from OWL 2 DL

OWL 2 RL covers only axioms that are expressible as SPARQL INSERT rules — it cannot generate
existential witnesses (new blank nodes) or perform the ABox/TBox separation that full DL
requires.  Use **OWL 2 DL** (with optional Konclude bridge) for:
- Complex cardinality constraints requiring witness generation.
- Full SROIQ(D) expressivity (nominals, role inversions, complex role chains at DL level).
- Soundness + completeness guarantees for classification.
