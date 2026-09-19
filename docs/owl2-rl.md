# OWL 2 RL Profile

OWL 2 RL (Rule Language) is a tractable sub-language of OWL 2 that maps cleanly to rule-based
forward chaining.  It runs 63 of the 78 OWL 2 RL/RDF rules of the W3C specification (OWL 2
Profiles §4.3, Tables 4–9); the 15 it does not run are listed below with their reason, and
`tests/owl2_rl_conformance.rs` pins both lists against the specification's inventory.

> **Open Triplestore role names:** class definitions and class axioms = graph role **Model** (the T-Box); ABox content = graph role **Instances**.  Property definitions and relations (the R-Box) belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning.  The RL reasoner reasons over the TBox+RBox schema together — the role split concerns where terms are stored and registered, not the reasoning semantics.

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
| prp-eqp1/2 | EquivalentProperty — subsumed: `scm-eqp1/2` turn it into mutual `rdfs:subPropertyOf` and `prp-spo1` propagates |
| prp-npa1/2 | NegativePropertyAssertion: inconsistency when assertion violated |
| prp-key | hasKey: merge individuals sharing the values of **every** key property — composite keys `owl:hasKey ( ex:first ex:last )` included (single-property keys were the only ones that fired before) |

### Class Rules (cls-*)

| Rule | Description |
|------|-------------|
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

### Datatype Rules (dt-*, Table 8)

| Rule | Description |
|------|-------------|
| dt-type1 | Every datatype of the OWL 2 RL datatype map (`xsd:integer`, `xsd:string`, `xsd:dateTime`, … — OWL 2 Profiles §4.2) is an `rdfs:Datatype` |
| dt-not-type | A literal whose lexical form is not in the lexical space of its datatype (`"abc"^^xsd:integer`) is an **inconsistency**; every XSD-typed literal in scope is checked with the lexical rules SHACL's `sh:datatype` uses |

`dt-type2`, `dt-eq` and `dt-diff` are not run: they type, equate or
distinguish literals *as subjects*, which an RDF graph cannot hold; literal
values are compared by SPARQL value semantics in every other rule.

### Schema Rules (scm-*)

| Rule | Description |
|------|-------------|
| scm-cls | Every class is subClassOf `owl:Thing` |
| scm-sco | SubClassOf transitivity |
| scm-eqc1/2 | EquivalentClass ↔ mutual subClassOf |
| scm-spo | SubPropertyOf transitivity |
| scm-eqp1/2 | EquivalentProperty ↔ mutual subPropertyOf |
| scm-dom1/2 | Domain inheritance through property and class hierarchies |
| scm-rng1/2 | Range inheritance through property and class hierarchies |
| scm-hv | HasValue schema entailment |
| scm-svf1/2 | SomeValuesFrom schema entailment |
| scm-avf | AllValuesFrom schema entailment |
| scm-int | Intersection schema entailment |
| scm-uni | Union schema entailment |

### Rules not run

The engine reports the two lists as `IMPLEMENTED_RULES` and
`UNIMPLEMENTED_RULES` (`src/reasoning/owl2_rl.rs`); together they are the
specification's 78 rules, and `tests/owl2_rl_conformance.rs` fails when they
are not.

| Rule | Why not |
|------|---------|
| eq-ref | reflexive `owl:sameAs` for every term of every triple: triples the graph, no other rule needs it |
| eq-diff2, eq-diff3 | `owl:AllDifferent` inconsistency: not implemented |
| prp-ap | the fixed list of annotation-property axiomatic triples: not implemented |
| prp-eqp1, prp-eqp2 | subsumed by `scm-eqp1/2` + `prp-spo1` |
| prp-pdw, prp-adp | `owl:propertyDisjointWith` / `owl:AllDisjointProperties` inconsistency: not implemented |
| cls-thing | every individual typed `owl:Thing`: one triple per term, no other rule needs it |
| cls-nothing1 | explicit `owl:Nothing` membership inconsistency: not implemented |
| dt-type2, dt-eq, dt-diff | need literal subjects (see above) |
| scm-op, scm-dp | reflexive `rdfs:subPropertyOf` / `owl:equivalentProperty` per property: no other rule needs it |

### Identity policy

Whether the Table 4 equality rules run at all for a dataset — and whether its
`linkset` graphs are premises — is the dataset's [identity policy](reasoning.md#identity-policy--what-happens-with-owlsameas)
(`sameas-off` / `sameas-narrow` / `sameas-full`). The raw engine
(`Owl2RLReasoner::new`) defaults to `sameas-full`; `with_identity_policy` selects.

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
