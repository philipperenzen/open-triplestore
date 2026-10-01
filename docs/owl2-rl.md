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

Each rule is a SPARQL `INSERT` (or an `ASK` for the inconsistency rules) run to
a fixed point. "⇒ false" means the run stops with an inconsistency error.

### Equality Rules (eq-*, Table 4)

| Rule | Description |
|------|-------------|
| eq-sym | `x sameAs y` ⇒ `y sameAs x` |
| eq-trans | `x sameAs y`, `y sameAs z` ⇒ `x sameAs z` |
| eq-rep-s / -p / -o | Copy every triple of `x` to each `y` it is `sameAs`, in subject, predicate and object position |
| eq-diff1 | `x sameAs y` and `x differentFrom y` ⇒ false |

### Property Rules (prp-*, Table 5)

| Rule | Description |
|------|-------------|
| prp-dom | `p rdfs:domain c`, `x p y` ⇒ `x type c` |
| prp-rng | `p rdfs:range c`, `x p y` ⇒ `y type c`, for IRI and blank-node objects only (a literal cannot be typed) |
| prp-fp | Functional property: two objects of one subject are `sameAs` |
| prp-ifp | Inverse functional property: two subjects of one object are `sameAs` |
| prp-irp | Irreflexive property: `x p x` ⇒ false |
| prp-symp | Symmetric property: `x p y` ⇒ `y p x` |
| prp-asyp | Asymmetric property: `x p y` and `y p x` ⇒ false |
| prp-trp | Transitive property: `x p y`, `y p z` ⇒ `x p z` (per round, so longer chains close over several rounds; `x p x` is never derived) |
| prp-spo1 | `p₁ rdfs:subPropertyOf p₂`, `x p₁ y` ⇒ `x p₂ y` |
| prp-spo2 | Property chain axiom with **two** properties: `p ← p₁ ∘ p₂` ⇒ `x p z`; longer chains are not applied |
| prp-inv1/2 | `p₁ owl:inverseOf p₂`: `x p₁ y` ⇔ `y p₂ x` |
| prp-key | hasKey: merge individuals sharing the values of **every** key property — composite keys `owl:hasKey ( ex:first ex:last )` included |
| prp-npa1/2 | A negative object or data property assertion that is contradicted by the data ⇒ false. The rule also requires the assertion node to be typed `owl:NegativePropertyAssertion`, which the specification does not |

`prp-eqp1/2` are not separate rules: `scm-eqp1` turns `owl:equivalentProperty`
into mutual `rdfs:subPropertyOf` and `prp-spo1` propagates (see the limit on
derived premises [below](#known-limits)).

### Class Rules (cls-*, Table 6)

| Rule | Description |
|------|-------------|
| cls-nothing2 | `x type owl:Nothing` ⇒ false |
| cls-int1 | `c` is the intersection of **two** classes and `x` is in both ⇒ `x type c`; intersections of three or more classes are not applied |
| cls-int2 | `x type c`, `c` an intersection ⇒ `x` is in every member class |
| cls-uni | `c` a union, `x` in a member class ⇒ `x type c` |
| cls-com | `c₁ owl:complementOf c₂`, `x` in both ⇒ false |
| cls-svf1 | `x owl:someValuesFrom c` on `p`, `u p v`, `v type c` ⇒ `u type x` |
| cls-svf2 | `x owl:someValuesFrom owl:Thing` on `p`, `u p v` ⇒ `u type x` |
| cls-avf | `x owl:allValuesFrom c` on `p`, `u type x`, `u p v` ⇒ `v type c`, for IRI and blank-node values only |
| cls-hv1/2 | `x owl:hasValue v` on `p`: `u type x` ⇒ `u p v`, and `u p v` ⇒ `u type x` |
| cls-maxc1 | `maxCardinality 0` on `p`, `u type x`, `u p y` ⇒ false |
| cls-maxc2 | `maxCardinality 1` on `p`, `u type x`, two values of `p` ⇒ the values are `sameAs` |
| cls-maxqc1–4 | The same two rules for `maxQualifiedCardinality 0` and `1`, with and without `owl:onClass owl:Thing` |
| cls-oo | `c owl:oneOf (x₁ … xₙ)` ⇒ each `xᵢ type c` |

### Class Axiom Rules (cax-*, Table 7)

| Rule | Description |
|------|-------------|
| cax-sco | SubClassOf: type inheritance |
| cax-eqc1/2 | EquivalentClass: bi-directional type inheritance |
| cax-dw | DisjointWith: an individual in both classes ⇒ false |
| cax-adc | AllDisjointClasses: expand to pairwise `owl:disjointWith` (IRI members only), then `cax-dw` applies |

### Datatype Rules (dt-*, Table 8)

| Rule | Description |
|------|-------------|
| dt-type1 | Every datatype of the OWL 2 RL datatype map (`xsd:integer`, `xsd:string`, `xsd:dateTime`, … — OWL 2 Profiles §4.2) is an `rdfs:Datatype` |
| dt-not-type | A literal whose lexical form is not in the lexical space of its own datatype (`"abc"^^xsd:integer`) is an **inconsistency**. It uses the lexical rules of SHACL's `sh:datatype`, which accept any lexical form for `xsd:token`, `xsd:language`, `xsd:Name`, `xsd:NCName`, `xsd:NMTOKEN`, `xsd:normalizedString`, `xsd:hexBinary`, `xsd:base64Binary` and `xsd:anyURI`. It does not check a literal against an `rdfs:range` or `owl:allValuesFrom` datatype |

`dt-type2`, `dt-eq` and `dt-diff` are not run: they type, equate or
distinguish literals *as subjects*, which an RDF graph cannot hold.

**Literals are matched as terms, not values.** Every rule joins literals with
SPARQL graph patterns, which compare RDF terms. The store canonicalises numbers,
booleans and dates within one datatype, so `"01"^^xsd:integer` and
`"1"^^xsd:integer` are the same term, but `"1"^^xsd:integer` and
`"1.0"^^xsd:decimal` are not, and neither are an `xsd:string` and an
`xsd:token` with the same text. `hasValue`, `hasKey`, `prp-npa2` and the
functional and cardinality rules therefore miss values that are equal but
written in different datatypes. Derived integer types (`xsd:byte`,
`xsd:nonNegativeInteger`, …) are stored as `xsd:integer`, so `dt-not-type`
never sees an out-of-range `"300"^^xsd:byte`.

### Schema Rules (scm-*, Table 9)

| Rule | Description |
|------|-------------|
| scm-cls | Every `owl:Class` is a subclass of itself and of `owl:Thing` (the specification's `c owl:equivalentClass c` and `owl:Nothing rdfs:subClassOf c` are not emitted) |
| scm-sco | SubClassOf transitivity |
| scm-eqc1/2 | EquivalentClass ↔ mutual subClassOf |
| scm-spo | SubPropertyOf transitivity |
| scm-eqp1/2 | EquivalentProperty ↔ mutual subPropertyOf |
| scm-dom1/2 | Domain inheritance through property and class hierarchies |
| scm-rng1/2 | Range inheritance through property and class hierarchies |
| scm-hv | HasValue schema entailment |
| scm-svf1/2 | SomeValuesFrom schema entailment |
| scm-avf1/2 | AllValuesFrom schema entailment |
| scm-int | An intersection is a subclass of each member (IRI members only) |
| scm-uni | Each member is a subclass of the union (IRI members only) |

### Known limits

- **Unscoped runs miss chained derivations.** Without a source scope (no
  `dataset` or `source_graphs` in `POST /api/reasoning/materialize`), the rules
  read the unnamed default graph, and only a few (`prp-dom`, `cax-sco`,
  `cls-maxc1`, `cax-dw`) also read the target graph where derived triples go.
  A rule whose premise was itself derived then does not fire: a 3-hop
  transitive chain, a 3-link `sameAs` chain, `owl:equivalentProperty`
  propagation, a range through a sub-property and most inconsistency checks on
  derived facts are missed. Dataset runs are scoped and include the target
  graph, so they are not affected.
- **Inverse property expressions** (`[ owl:inverseOf ex:p ]` in a restriction,
  chain or key) would need a blank-node predicate, which RDF cannot store; rules
  over them match nothing.
- **Inconsistency is an HTTP 500.** The reasoning endpoint does not yet tell the
  client which rule failed.
- **Iteration cap.** The loop stops after 500 rounds without saying so.

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
| cls-thing | the axiomatic triple `owl:Thing rdf:type owl:Class`: not emitted |
| cls-nothing1 | the axiomatic triple `owl:Nothing rdf:type owl:Class`: not emitted (an individual typed `owl:Nothing` is caught by `cls-nothing2`) |
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
use std::path::Path;

let store = TripleStore::open(Path::new("./data"))?;
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

The fixed-point loop runs every rule as a SPARQL update each round until nothing new is
derived, so the number of rounds grows with the depth of the class and property hierarchies
and the length of `sameAs` and transitive chains. No benchmark of the reasoner is published.

## Differences from OWL 2 DL

OWL 2 RL covers only axioms that are expressible as SPARQL INSERT rules — it cannot generate
existential witnesses (new blank nodes) or perform the ABox/TBox separation that full DL
requires.  Those need a complete OWL 2 DL reasoner, which this project does not ship: the
`owl2-dl` regime adds only a few DL rules on top of RL (see [OWL 2 DL](owl2-dl.md)). A complete
reasoner is what you need for:
- Complex cardinality constraints requiring witness generation.
- Full SROIQ(D) expressivity (nominals, role inversions, complex role chains at DL level).
- Soundness + completeness guarantees for classification.
