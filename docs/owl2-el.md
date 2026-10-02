# OWL 2 EL Profile

OWL 2 EL (Existential Language) is a tractable sub-language of OWL 2 designed for large
biomedical ontologies such as SNOMED CT, GO, and NCI Thesaurus.

> **Open Triplestore role names:** class definitions and class axioms = graph role **Model** (the T-Box); ABox content = graph role **Instances**.  Property definitions and relations (the R-Box) belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning.  The EL reasoner classifies over the TBox+RBox schema together — the role split concerns where terms are stored and registered, not the reasoning semantics.

The classifier here is a set of SPARQL `INSERT` rules run to a fixed point. It
covers part of the profile (the [Standards](standards.md) page grades it
**Partial**). It reads:

- `rdfs:subClassOf` and `owl:equivalentClass` (both directions, named classes
  and class expressions alike)
- `owl:intersectionOf` (two-operand lists for the joins below)
- existential restrictions (`owl:someValuesFrom` with `owl:onProperty`)
- `rdfs:subPropertyOf`, `owl:equivalentProperty`, `owl:TransitiveProperty` and
  `owl:ReflexiveProperty`
- property chains of two or three properties (`owl:propertyChainAxiom`)
- `rdfs:domain`, `rdfs:range` and `owl:disjointWith`
- `owl:hasKey`, with any number of key properties

It does not read `owl:hasValue`, nominals (`owl:oneOf`), `owl:hasSelf` or the
EL datatypes; axioms it does not read are ignored without a warning.

## Completion Rules Implemented

The rule names are this implementation's own; they do not follow the CR
numbering of the EL++ literature. `x type C` is `x rdf:type C`, and every
derived triple is written to the target graph.

| Rule | What it derives |
|------|-----------------|
| EQC | `A ≡ B` ⇒ `A ⊑ B` and `B ⊑ A` |
| CR1 | `A ⊑ B`, `B ⊑ C` ⇒ `A ⊑ C` |
| CR2 | `C ≡ B₁ ⊓ … ⊓ Bₙ` ⇒ `C ⊑ Bᵢ`; for two operands, `A ⊑ B₁` and `A ⊑ B₂` ⇒ `A ⊑ C` |
| ROLE | `p ≡ q` ⇒ `p ⊑ q`, `q ⊑ p`; `p ⊑ q`, `q ⊑ r` ⇒ `p ⊑ r` |
| CR4 | `A ⊑ ∃r.B`, `B ⊑ C` (or `C = owl:Thing`), `r ⊑ s` ⇒ `A ⊑ ∃s.C` for every restriction `∃s.C` in scope; two restrictions on the same property and filler are the same class |
| CR5 | Two-property chain: `p ← p₁ ∘ p₂`, `x p₁ y`, `y p₂ z` ⇒ `x p z` |
| CR6 | `A ⊑ D`, `D ⊑ ⊥` ⇒ `A ⊑ ⊥`; `A ⊑ ∃r.B`, `B ⊑ ⊥` ⇒ `A ⊑ ⊥`; `A ⊑ B`, `A ⊑ C`, `B` disjoint with `C` ⇒ `A ⊑ ⊥` |
| CR7 | `p rdfs:domain A`, `x p y` ⇒ `x type A` |
| CR8 | `p rdfs:range A`, `x p y` (y an IRI or blank node) ⇒ `y type A` |
| CR9 | `p` reflexive ⇒ `x p x` for every IRI `x` that is the subject of a triple |
| CR10 | Three-property chain: `p ← p₁ ∘ p₂ ∘ p₃` ⇒ `x p w` |
| ABox | `x type C`, `C ⊑ D` ⇒ `x type D`; two-operand intersection membership; `x p y`, `y type B` (or `B = owl:Thing`) ⇒ `x type ∃p.B`; `x p y`, `p ⊑ q` ⇒ `x q y`; `p` transitive ⇒ `x p z` from `x p y`, `y p z` |
| hasKey | `C hasKey (p₁ … pₙ)`, `x` and `y` of type `C` share a value of every `pᵢ` ⇒ `x owl:sameAs y` |

There is no rule from `A ⊑ ∃p.B` and `B ⊑ C` to a subsumption *into* `A`.
An earlier CR3 derived `∃p.C ⊑ A`, which does not follow; with the ABox
existential rule it typed any `x p y, y type C` as an `A`.

## Consistency

After the fixed point `classify()` checks consistency and fails with
`ReasoningError::Inconsistency` (HTTP 500 from `POST /api/reasoning/materialize`)
when an individual is an instance of `owl:Nothing` (directly or through its
classes), an individual is an instance of two disjoint classes, or
`owl:Thing ⊑ owl:Nothing`. What was derived stays in the target graph. An
unsatisfiable class without instances is not an inconsistency;
`El2Classifier::unsatisfiable_classes()` lists those. Set
`detect_inconsistency = false` to skip the check.

A run without a dataset or source graphs reads only the unnamed default graph,
so a rule whose premise was itself derived into the target graph does not
fire there (a transitive chain longer than two, `A ⊑ ∃r.B ⊑ … ⊑ D`). Dataset
runs read the target graph too.

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

let store = TripleStore::open("./data")?;
let reasoner = El2Classifier::new(&store);
let report = reasoner.classify()?;

println!(
    "OWL 2 EL: {} triples in {} iterations ({} ms)",
    report.triples_added, report.iterations, report.elapsed_ms
);
```

Entailed triples are stored in `urn:entailment:owl2-el`.

## Typical Use Cases

- **Biomedical ontologies**: SNOMED CT (360k+ concepts), Gene Ontology (40k+ terms), NCI
  Thesaurus — all fit within the EL profile.
- **Taxonomy hierarchies**: Any large multi-level classification where intersection and
  existential restrictions are needed but nominals and transitive properties at the full DL
  level are not.
- **Publishing pipelines**: Pre-classify an ontology at ingest time and query the materialised
  hierarchy without a live reasoner.

## Performance

EL reasoning scales linearly in the number of axioms.  On a MacBook M3 Pro:

| Ontology | Triples | Inferred | Time |
|----------|---------|----------|------|
| GO (Gene Ontology) | ~200k | ~80k | ~0.3s |
| SNOMED CT core (sample) | ~1M | ~600k | ~2.5s |

## Differences from OWL 2 RL

OWL 2 EL focuses on forward classification via completion rules; OWL 2 RL uses a broader set of
~80 SPARQL INSERT rules covering more of the RDF-based OWL semantics.  In practice:

- Use **EL** for large taxonomy/ontology classification (fast, scales to millions of axioms).
- Use **RL** for rule-based reasoning over ABox data that uses OWL 2 RL-expressible axioms.
- Use **DL** when you need full SROIQ(D) expressivity (requires Konclude or similar).
