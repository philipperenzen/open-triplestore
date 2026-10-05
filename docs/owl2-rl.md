# OWL 2 RL Profile

OWL 2 RL (Rule Language) is a tractable sub-language of OWL 2 that maps cleanly to rule-based
forward chaining.  It runs all 78 OWL 2 RL/RDF rules of the W3C specification (OWL 2
Profiles §4.3, Tables 4–9), and `tests/owl2_rl_conformance.rs` pins the list against the
specification's inventory. The three Table 8 rules whose conclusions have a literal subject
are applied to the literals' data values (see [Datatype rules](#datatype-rules-dt--table-8)).
`eq-ref` runs only when asked for (see [eq-ref](#eq-ref-is-opt-in)).

> **Open Triplestore role names:** class definitions and class axioms = graph role **Model** (the T-Box); ABox content = graph role **Instances**.  Property definitions and relations (the R-Box) belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning.  The RL reasoner reasons over the TBox+RBox schema together — the role split concerns where terms are stored and registered, not the reasoning semantics.

OWL 2 RL is suitable for:
- Reasoning over large ABoxes with moderate TBoxes.
- Scenarios where all axioms are expressible as SPARQL INSERT rules (no existential witnesses
  needed, no full tableau).
- Enterprise knowledge graphs using property hierarchies, class hierarchies, cardinality
  constraints, and property chains.

## Rules Implemented

Lists — `owl:intersectionOf`, `owl:unionOf`, `owl:oneOf`, `owl:propertyChainAxiom`,
`owl:hasKey`, `owl:members`, `owl:distinctMembers` — may have any length and any members,
blank-node class expressions included.

### Equality Rules (eq-*)

| Rule | Description |
|------|-------------|
| eq-ref | Every term is `owl:sameAs` itself — **opt-in**, see below |
| eq-sym, eq-trans | `owl:sameAs` is symmetric and transitive |
| eq-rep-s/p/o | Replace a term by an `owl:sameAs` term in subject, predicate or object position |
| eq-diff1 | `x owl:sameAs y` and `x owl:differentFrom y` is an **inconsistency** (so is `x owl:differentFrom x`) |
| eq-diff2, eq-diff3 | Two members of an `owl:AllDifferent` (`owl:members` / `owl:distinctMembers`) that are `owl:sameAs`, or one individual listed twice, is an **inconsistency** |

### Property Rules (prp-*)

| Rule | Description |
|------|-------------|
| prp-ap | The built-in annotation properties (`rdfs:label`, `rdfs:comment`, `rdfs:seeAlso`, `rdfs:isDefinedBy`, `owl:deprecated`, `owl:versionInfo`, `owl:priorVersion`, `owl:backwardCompatibleWith`, `owl:incompatibleWith`) are `owl:AnnotationProperty` |
| prp-dom | Property domain: `?p rdfs:domain ?c` → type subjects |
| prp-rng | Property range: `?p rdfs:range ?c` → type objects |
| prp-fp | Functional property: merge objects sharing same subject |
| prp-ifp | Inverse functional property: merge subjects sharing same object |
| prp-irp | Irreflexive property: detect `x P x` as inconsistency |
| prp-symp | Symmetric property: if `x P y` then `y P x` |
| prp-asyp | Asymmetric property: detect `x P y` AND `y P x` as inconsistency |
| prp-trp | Transitive property: `x P y`, `y P z` → `x P z`; a cycle gives `x P x` |
| prp-spo1 | SubPropertyOf: propagate triples through superproperty |
| prp-spo2 | Property chain axiom `P ← P1 ∘ … ∘ Pn`, any length |
| prp-eqp1/2 | EquivalentProperty: propagate triples both ways |
| prp-pdw | `owl:propertyDisjointWith`: two disjoint properties linking the same pair is an **inconsistency** |
| prp-adp | `owl:AllDisjointProperties`: two of its members linking the same pair is an **inconsistency** |
| prp-inv1/2 | InverseOf: `x P y` ↔ `y Q x` |
| prp-npa1/2 | NegativePropertyAssertion: inconsistency when assertion violated (the three NPA properties are enough; no `rdf:type` triple needed) |
| prp-key | hasKey: merge named individuals sharing the values of **every** key property — composite keys and inverse key properties included |

### Class Rules (cls-*)

| Rule | Description |
|------|-------------|
| cls-thing, cls-nothing1 | `owl:Thing` and `owl:Nothing` are `owl:Class` |
| cls-nothing2 | An instance of `owl:Nothing` (asserted or derived) is an **inconsistency** |
| cls-int1 | Intersection membership: `x type C` if `x` is a member of every class in `C owl:intersectionOf (C1 … Cn)` |
| cls-int2 | Intersection decomposition: members of `C1 ∩ … ∩ Cn` are members of each |
| cls-uni | Union: a member of any `Ci` is a member of `C owl:unionOf (C1 … Cn)` |
| cls-com | ComplementOf inconsistency: `x type C` and `x type ¬C` |
| cls-svf1/2 | SomeValuesFrom: `u P v`, `v type D` → `u type ∃P.D` (`D = owl:Thing`: any `u P v`) |
| cls-avf | AllValuesFrom: propagate range restrictions |
| cls-hv1/2 | HasValue: property assertions from value restrictions, and back |
| cls-maxc1 | MaxCardinality 0: any `u P y` for an instance `u` is an **inconsistency** |
| cls-maxc2 | MaxCardinality 1: two values of an instance are `owl:sameAs` |
| cls-maxqc1-4 | Qualified MaxCardinality 0 (**inconsistency**) and 1 (`owl:sameAs`) |
| cls-oo | OneOf: each listed individual is a member |

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

| dt-type2 | Every literal is of each datatype of the map whose value space holds its value: `"3"^^xsd:integer` is also an `xsd:decimal`, `xsd:byte`, … (never an `xsd:double`: the float and double value spaces are disjoint from the decimal one) |
| dt-eq | Literals with equal values (`"1"^^xsd:integer`, `"1.0"^^xsd:decimal`) are the same |
| dt-diff | Literals with different values are different |

The three rules conclude triples with a literal subject, which an RDF graph cannot hold, so
the engine draws their RDF-representable consequences itself, from the data values of the
literals in scope (the OWL 2 RL datatype map of `src/reasoning/datatypes.rs`):

- **Equal values.** A literal written two ways gets the other's triples (`dt-eq` with
  `eq-rep-o`): the *variants* are written to the target graph. `owl:hasValue` (`cls-hv1/2`),
  `owl:hasKey` (`prp-key`) and negative data assertions (`prp-npa2`) therefore match by value.
- **Typing.** A literal has its `dt-type2` datatypes plus the classes `rdfs:range` (`prp-rng`)
  and `owl:allValuesFrom` (`cls-avf`) give it, closed under `rdfs:subClassOf`,
  `owl:equivalentClass` and `owl:intersectionOf`. A `owl:someValuesFrom` restriction on a data
  property types its subject from that (`cls-svf1`).
- **Inconsistencies.** A literal outside a datatype it is typed with (`:age rdfs:range
  xsd:integer . :x :age "forty"`) is `dt-not-type`; two different values of a functional data
  property, or of a `owl:maxCardinality 1` / `owl:maxQualifiedCardinality 1` restriction, are
  `dt-diff` (with `eq-diff1`); a literal in `owl:Nothing`, two disjoint or complementary
  classes, or counted by a maximum qualified cardinality of 0 is reported by the class rule.

Equal-valued variants are written whatever the [identity policy](#identity-policy): the
policy concerns `owl:sameAs` between individuals, not the values of literals.

Not simulated: conclusions reached *through* a triple with a literal subject. They arise only
when a data property is used as an object property (declared symmetric, transitive or the
inverse of another), which OWL 2's typing rules out, and from `owl:sameAs` between a literal
and an IRI.

Storage limit: Oxigraph stores the integer-derived types (`xsd:byte` …
`xsd:nonNegativeInteger`) as `xsd:integer` and `xsd:dateTimeStamp` as `xsd:dateTime`, so
`"300"^^xsd:byte` reaches the reasoner as the integer 300 and is not flagged as ill-typed
(decision D4: a documented limit, no ingest-time check). A range or restriction naming
`xsd:byte` still rejects the value 300.

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

### Schema Rules (scm-*)

| Rule | Description |
|------|-------------|
| scm-cls | Every class `C` is `C ⊑ C`, `C ≡ C`, `C ⊑ owl:Thing` and `owl:Nothing ⊑ C` |
| scm-sco | SubClassOf transitivity |
| scm-eqc1/2 | EquivalentClass ↔ mutual subClassOf |
| scm-op, scm-dp | Every object / datatype property `P` is `P ⊑ P` and `P ≡ P` |
| scm-spo | SubPropertyOf transitivity |
| scm-eqp1/2 | EquivalentProperty ↔ mutual subPropertyOf |
| scm-dom1/2 | Domain inheritance through property and class hierarchies |
| scm-rng1/2 | Range inheritance through property and class hierarchies |
| scm-hv | HasValue schema entailment |
| scm-svf1/2 | SomeValuesFrom schema entailment |
| scm-avf1/2 | AllValuesFrom schema entailment |
| scm-int | Intersection schema entailment |
| scm-uni | Union schema entailment |

### Inverse property expressions

Wherever a rule reads a property — domain, range, characteristics, sub- and
equivalent properties, chains, keys, disjoint properties, negative property
assertions and the `owl:onProperty` of a restriction — it may be an inverse
property expression `[ owl:inverseOf P ]`:

```turtle
ex:sibling owl:propertyChainAxiom ( ex:hasParent [ owl:inverseOf ex:hasParent ] ) .
[ owl:inverseOf ex:hasParent ] rdfs:subPropertyOf ex:hasChild .
ex:Pet rdfs:subClassOf [ owl:onProperty [ owl:inverseOf ex:owns ] ; owl:allValuesFrom ex:Owner ] .
```

The expression is a blank node, and no RDF triple can have a blank-node predicate, so a premise
`u [inverseOf P] v` is read as `v P u` and a consequence `x [inverseOf P] y` is written as
`y P x`.

### eq-ref is opt-in

`eq-ref` writes `x owl:sameAs x` for every subject, predicate and non-literal object: about one
triple per term, and no other rule needs those triples to fire. It is off by default; turn it
on with `Owl2RLReasoner::with_eq_ref(true)` or `"eq_ref": true` in the body of
`POST /api/reasoning/materialize`. `sameas-off` skips it with the other equality rules. The
inconsistencies that follow from it — `x owl:differentFrom x`, an individual listed twice in an
`owl:AllDifferent` — are reported whether or not it runs. Without it, the only triples missing
from the closure are the reflexive `owl:sameAs` ones. The grade (Full, docs/standards.md) is
given with `eq-ref` off by default (decision D2).

### The rule inventory

The engine lists the rules it runs as `IMPLEMENTED_RULES` (`src/reasoning/owl2_rl.rs`), all 78
of the specification; `UNIMPLEMENTED_RULES` is empty, and `tests/owl2_rl_conformance.rs` fails
when the two lists are not exactly the specification's rules.

### How the closure is checked

- `tests/owl2_rl_conformance.rs` has a test per rule group, and a differential test: a
  test-only evaluator (`tests/support/rl_reference.rs`) applies the 78 rules naively over
  *generalized* triples (literal subjects included, so `dt-type2` / `dt-eq` / `dt-diff` run as
  written), and for seeded random graphs the engine must agree with it on consistency and,
  when consistent, produce exactly the RDF-representable part of its closure (reflexive
  `owl:sameAs` aside: those are `eq-ref`'s).
- `tests/w3c_owl2_rl_manifests.rs` runs the approved W3C OWL 2 test cases of the RL profile;
  `tests/w3c_sparql11_entailment_manifests.rs` runs the SPARQL 1.1 entailment-regime cases
  for the OWL 2 RDF-Based Semantics. Known gaps are in
  [conformance/owl2-rl.md](conformance/owl2-rl.md); no score is published (W3C test-suite
  policy).

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
reasoner.check_consistency()?;   // Err(ReasoningError::Inconsistency { rule, detail }) if violated
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
requires.  Use the **OWL 2 DL** regime with a complete backend (the bundled OWL API + HermiT
reasoner sidecar, or Konclude; see [OWL 2 DL](owl2-dl.md)) for:
- Complex cardinality constraints requiring witness generation.
- Full SROIQ(D) expressivity (nominals, role inversions, complex role chains at DL level).
- Soundness + completeness guarantees for classification.
