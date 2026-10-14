# OWL 2 EL Profile

OWL 2 EL (Existential Language) is a tractable sub-language of OWL 2 designed for large
biomedical ontologies such as SNOMED CT, GO, and NCI Thesaurus.

> **Open Triplestore role names:** class definitions and class axioms = graph role **Model** (the T-Box); ABox content = graph role **Instances**.  Property definitions and relations (the R-Box) belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning.  The EL reasoner classifies over the TBox+RBox schema together — the role split concerns where terms are stored and registered, not the reasoning semantics.

The EL reasoner is a native EL++ saturation engine (the completion rules of
Baader, Brandt and Lutz, as in ELK). It reads the ontology straight out of the
quad index, normalizes it, saturates it with a worklist, and writes what follows
into the target graph. It covers the whole OWL 2 EL profile; the
[Standards](standards.md) page grades it **Full**.

## What it derives

| Output | Triples written |
|--------|-----------------|
| Classification | `C rdfs:subClassOf D` from every class IRI to every class IRI and EL class expression that subsumes it; `C owl:equivalentClass D` between equivalent class IRIs; `C rdfs:subClassOf owl:Nothing` for an unsatisfiable class; `owl:Thing rdfs:subClassOf D` when `D` is equivalent to `owl:Thing` |
| Property hierarchy | `p rdfs:subPropertyOf q` (and `owl:equivalentProperty`), closed under inclusion |
| Realization | `x rdf:type C` for every individual and every class IRI it belongs to |
| Property assertions | `x p y` between individuals, and to data values, through sub-properties, property chains of any length, transitivity, reflexivity, self restrictions, `owl:hasValue` and equality |
| Equality | `x owl:sameAs y` between individuals that `owl:sameAs`, `owl:hasKey` or a one-individual `owl:oneOf` make the same |

Only triples that are not already in a premise graph are written. They are
written with one `insert_quads` call, so the graph index and change capture see
them like any other write.

## Supported constructs

Everything in the OWL 2 EL profile (OWL 2 Profiles §2.2):

- **Class expressions:** class IRIs, `owl:Thing`, `owl:Nothing`,
  `owl:intersectionOf` of any arity, `owl:someValuesFrom`, `owl:hasValue` and
  `owl:hasSelf` over object properties, `owl:oneOf` of one individual, and
  `owl:someValuesFrom` and `owl:hasValue` over data properties.
- **Data ranges:** the nineteen datatypes of the EL datatype map (below), datatypes
  the ontology declares (`rdfs:Datatype`, with an `owl:equivalentClass`
  definition), `owl:intersectionOf` of data ranges, and `owl:oneOf` of one literal.
- **Class axioms:** `rdfs:subClassOf`, `owl:equivalentClass`, `owl:disjointWith`,
  `owl:AllDisjointClasses`, `owl:hasKey`.
- **Property axioms:** `rdfs:subPropertyOf`, `owl:equivalentProperty`,
  `owl:propertyChainAxiom`, `owl:TransitiveProperty`, `owl:ReflexiveProperty`,
  `rdfs:domain`, `rdfs:range` for object and data properties, and
  `owl:FunctionalProperty` on data properties.
- **Assertions:** class and property assertions, `owl:sameAs`, `owl:differentFrom`,
  `owl:AllDifferent`, and `owl:NegativePropertyAssertion` (object and data).

Every class expression is defined by its structure. Two blank nodes that spell
`∃p.C` are two names for one class, and the rules work that out.

### Data values

The EL datatypes are `rdfs:Literal`, `rdf:PlainLiteral`, `rdf:XMLLiteral`,
`owl:real`, `owl:rational`, `xsd:decimal`, `xsd:integer`,
`xsd:nonNegativeInteger`, `xsd:string`, `xsd:normalizedString`, `xsd:token`,
`xsd:Name`, `xsd:NCName`, `xsd:NMTOKEN`, `xsd:hexBinary`, `xsd:base64Binary`,
`xsd:anyURI`, `xsd:dateTime` and `xsd:dateTimeStamp`.

- **Value semantics.** Literals compare by value: `"1"^^xsd:integer` and
  `"1.0"^^xsd:decimal` are the same number, `"a"` and `"a"^^xsd:token` are the
  same string, and two `xsd:dateTime`s with time zones are equal when they are the
  same instant.
- **Datatype membership** comes from the value. `42` is also an
  `xsd:nonNegativeInteger`, and `"abc"` is also an `xsd:NCName`.
- **Disjointness.** Numbers, strings, date-times, the two binary types, IRIs and
  XML literals are pairwise disjoint.
- **Ill-typed literals.** A literal whose lexical form is not valid for its EL
  datatype makes the ontology inconsistent.
- **Storage limit.** The store keeps every integer-derived XSD type as
  `xsd:integer` and `xsd:dateTimeStamp` as `xsd:dateTime`. So
  `"-5"^^xsd:nonNegativeInteger` reaches the reasoner as the integer −5 and is not
  reported as ill-typed.

### What is left out, and reported

A construct outside the profile is kept as an opaque class: the reasoner only
loses consequences, it never adds a wrong one. Such constructs include:

- unions, complements and universals;
- cardinalities;
- inverse properties;
- functional, inverse-functional, symmetric, asymmetric and irreflexive object
  properties;
- disjoint properties and `owl:disjointUnionOf`;
- `owl:oneOf` with more than one member;
- facets (datatype restrictions) and datatypes outside the EL map.

The report counts these per construct, with the first one read as the
example (an IRI, or a blank-node label for an anonymous expression), the most
frequent construct first. OWL 2 QL reports in the same shape:

```json
"ignored": [
  { "construct": "ObjectUnionOf", "count": 1, "example": "_:b0" }
]
```

`ReasoningReport::ignored` carries the same list in Rust.
`POST /api/reasoning/materialize` includes it only when it is not empty.

## How it works

1. **Load.** The engine reads every quad of the premise graphs and interns the
   terms. Every class expression gets a concept id and axioms that define it.
   Longer intersections and property chains are split into binary ones with fresh
   names. Each individual and each distinct data value becomes a *nominal*: a
   context that stands for exactly one element.
2. **Saturate.** Every class, individual and value has a context: the set
   `S(X)` of concepts that subsume it, plus its links `X →r Y` ("every `X` has an
   `r`-successor in `Y`"). Contexts for existential fillers are shared, which keeps
   the closure polynomial. A worklist applies these rules until nothing new
   follows:

   | Rule | From | Derives |
   |------|------|---------|
   | CR1 | `A ∈ S(X)`, `A ⊑ B` | `B ∈ S(X)` |
   | CR2 | `A₁, A₂ ∈ S(X)`, `A₁ ⊓ A₂ ⊑ B` | `B ∈ S(X)` |
   | CR3 | `A ∈ S(X)`, `A ⊑ ∃r.B` | `X →r B` (or `X →r (B ⊓ ran(r))` when `r` has a range) |
   | CR4 | `X →r Y`, `A ∈ S(Y)`, `∃r.A ⊑ B` | `B ∈ S(X)` |
   | CR5 | `X →r Y`, `⊥ ∈ S(Y)` | `⊥ ∈ S(X)` |
   | CR10 | `X →r Y`, `r ⊑ s` | `X →s Y` |
   | CR11 | `X →r Y`, `Y →s Z`, `r ∘ s ⊑ t` | `X →t Z` |
   | RNG | `X →r y` with `y` an individual or value | `ran(r) ∈ S(y)` |
   | REF | `r` reflexive | a self loop `X →r X` and `ran(r) ∈ S(X)` for every `X` |
   | SELF | `A ⊑ ∃r.Self` / a self loop and `∃r.Self ⊑ B` | a self loop / `B ∈ S(X)` |
   | NOM-in | `{a} ∈ S(X)` | `S({a}) ⊆ S(X)`, and `a`'s links and loops are `X`'s |
   | NOM-out | `{a} ∈ S(X)`, `X` non-empty in every model | `X ∈ S({a})` |
   | FUN | `X →f Y₁`, `X →f Y₂`, `f` functional | `X →f (Y₁ ⊓ Y₂)` |
   | DATA | two values, or a value and a datatype without it, in `S(X)` | `⊥ ∈ S(X)` |

   A context is *non-empty* when it is `owl:Thing`, a nominal, or reachable from
   one. An edge between two individuals that are the same element is a self loop.
   Transitivity is the chain `r ∘ r ⊑ r`. Keys run between saturation rounds:
   individuals that agree on a key become the same, and the next round can make
   more of them agree.
3. **Classes and nominals.** NOM-in and NOM-out are complete for individuals.
   A class can still miss a subsumption when two contexts reachable from it are
   forced to be the same individual once it has an instance; ELK leaves this case
   out. The engine detects such a class and classifies it the textbook way:
   `C ⊑ D` holds iff `h : D` follows for a fresh individual `h : C`. That check
   saturates a copy of the state, so it costs one extra saturation per affected
   class. Ontologies without nominals in class expressions never pay it.
4. **Write.** The engine turns the contexts back into the triples listed above,
   skips those already in a premise graph, and inserts the rest.

## Consistency

`classify()` fails with `ReasoningError::Inconsistency` (HTTP 500 from
`POST /api/reasoning/materialize`) after writing what it derived, when:

- `owl:Thing ⊑ owl:Nothing`;
- an individual is an instance of `owl:Nothing` (directly, through its classes and
  successors, or by being typed with two disjoint classes);
- an individual is the same as one it is different from;
- an individual has a property value that a negative property assertion denies;
- a literal is ill-typed or outside a data range it must be in (including two
  values of a functional data property).

The error message names the individual or literal.

An unsatisfiable class without instances is not an inconsistency.
`El2Classifier::unsatisfiable_classes()` lists those, and
`check_consistency()` answers without writing. Set `detect_inconsistency = false`
to skip the check.

The rules read the source graphs (the unnamed default graph when the run is
unscoped) plus the target graph. A rule always sees its own earlier
consequences.

## Configuration

```toml
# Cargo.toml
[features]
owl2-el = ["rdfs-entailment"]
```

Or as part of the full feature set:

```toml
open-triplestore = { features = ["full"] }
```

## API Usage

```rust
use open_triplestore::reasoning::owl2_el::El2Classifier;
use open_triplestore::store::TripleStore;
use std::path::Path;

let store = TripleStore::open(Path::new("./data"))?;
let report = El2Classifier::new(&store)
    .with_sources(vec!["urn:example:ontology".to_string()])
    .classify()?;

println!(
    "OWL 2 EL: {} new triples in {} rounds ({} ms), {} constructs ignored",
    report.triples_added, report.iterations, report.elapsed_ms, report.ignored.len()
);
```

Entailed triples are stored in `urn:entailment:owl2-el` unless `with_target`
names another graph.

## Typical Use Cases

- **Biomedical ontologies:** SNOMED CT, the Gene Ontology and the NCI Thesaurus
  all fall within the EL profile.
- **Taxonomies:** any large multi-level classification that uses intersections and
  existential restrictions.
- **Publishing pipelines:** classify an ontology once at ingest and query the
  materialised hierarchy without a live reasoner.

## Performance

**What was measured.** The saturation core (`src/reasoning/owl2_el/saturate.rs`),
compiled with optimisations (`rustc -O`) and run single-threaded on an Apple
Silicon laptop under heavy load from other builds (load average 15–30). The
inputs were synthetic, SNOMED-shaped ontologies:

- a random poly-hierarchy;
- a third of the classes fully defined as `parent ⊓ ∃r.F` with one or two
  existentials;
- 40 roles in a random hierarchy;
- a transitive `partOf` and one right-identity chain;
- optionally, individuals that each have one type and up to two edges into a
  forest.

| Classes | Individuals | Subsumers derived | Types derived | Time | Peak memory |
|--------:|------------:|------------------:|--------------:|-----:|------------:|
| 10 000 | — | 0.40 M | — | 0.17 s | 39 MB |
| 50 000 | — | 2.8 M | — | 1.9 s | 222 MB |
| 100 000 | — | 6.5 M | — | 4.9 s | 426 MB |
| 20 000 | 50 000 | 0.92 M | 2.6 M | 1.7 s | 241 MB |

**What these numbers leave out.** Loading the quads and writing the result add to
these times. Writing usually costs more, since every derived triple becomes a
quad. Those parts were not measured in a release build, and no real SNOMED CT or
GO release was run.

**How it scales.** Time and memory grow with the number of derived subsumptions
(`classes × subsumers per class`), not just with the input size. Under a
transitive property the property-assertion closure is quadratic in the length
of its chains: materialising every `partOf` pair of a deep part hierarchy is
large however it is computed.

A dataset in `materialize` mode re-runs the whole classification after every
write that touches its graphs.

## Differences from OWL 2 RL

- **EL** computes the whole classification, including subsumptions that need an
  anonymous individual (`A ⊑ ∃r.B`, `B ⊑ C`, `∃r.C ⊑ D` ⊨ `A ⊑ D`), in polynomial
  time.
- **RL** applies the RL/RDF rules to the triples and is complete for atomic facts
  about the individuals of an RL ontology.

On ontologies in both profiles the two agree on every type and property
assertion of the individuals. Two randomised differential tests in
`tests/owl2_el_conformance.rs` check this: one plain, one with `owl:hasValue`,
`owl:sameAs` and keys.

In practice:

- Use **EL** for large taxonomies and ontology classification.
- Use **RL** for rule-style reasoning over ABox data with RL axioms (universals in
  superclass positions, functional and inverse properties).
- Use **DL** when you need full SROIQ(D) expressivity (requires Konclude or similar).
