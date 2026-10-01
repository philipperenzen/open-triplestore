# OWL 2 EL Profile

OWL 2 EL (Existential Language) is a tractable sub-language of OWL 2 designed for large
biomedical ontologies such as SNOMED CT, GO, and NCI Thesaurus.

> **Open Triplestore role names:** class definitions and class axioms = graph role **Model** (the T-Box); ABox content = graph role **Instances**.  Property definitions and relations (the R-Box) belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning.  The EL reasoner classifies over the TBox+RBox schema together — the role split concerns where terms are stored and registered, not the reasoning semantics.

The reasoner here is a set of SPARQL `INSERT` rules run to a fixed point. It
covers part of the EL profile; the [Standards](standards.md) page grades it
**Partial**. It reads:

- `rdfs:subClassOf` and `owl:intersectionOf` (two-operand lists for the joins below)
- existential restrictions (`owl:someValuesFrom` with `owl:onProperty`)
- property chains of two or three properties (`owl:propertyChainAxiom`)
- `rdfs:domain`, `rdfs:range` and `owl:ReflexiveProperty`
- `owl:hasKey` with a single key property

It does **not** read `owl:equivalentClass`, `rdfs:subPropertyOf`,
`owl:equivalentProperty`, `owl:TransitiveProperty`, `owl:hasValue`, nominals
(`owl:oneOf`), `owl:hasSelf`, `owl:disjointWith` or the EL datatypes. Axioms
outside what it reads are ignored without a warning.

OWL 2 EL as a profile is PTIME-complete; this rule-based implementation is not
an EL++ saturation engine and makes no scaling claim of its own.

## Rules implemented

The rule names are the ones in `src/reasoning/owl2_el.rs`; they do not follow
the CR numbering of the EL++ literature. `x type C` is `x rdf:type C`, and every
derived triple is written to the target graph.

| Rule | What it derives |
|------|-----------------|
| CR1 | `A ⊑ B`, `B ⊑ C` ⇒ `A ⊑ C` (subClassOf transitivity) |
| CR2 | `C ≡ B₁ ⊓ … ⊓ Bₙ` ⇒ `C ⊑ Bᵢ` for each operand; for a two-operand intersection, `A ⊑ B₁` and `A ⊑ B₂` ⇒ `A ⊑ C` |
| CR3 | `A ⊑ ∃p.B`, `B ⊑ C`, and a restriction `∃p.C` in the data ⇒ `∃p.C ⊑ A`. **This inference is unsound** (it should be `A ⊑ ∃p.C`), and together with the ABox existential rule below it can type an individual wrongly. A fix is planned. |
| CR4 | `A ⊑ R`, `R ⊑ C` where `R` is a `someValuesFrom` restriction ⇒ `A ⊑ C` |
| CR5 | Two-property chain: `p ← p₁ ∘ p₂`, `x p₁ y`, `y p₂ z` ⇒ `x p z` |
| CR6 | `C ⊑ D`, `D ⊑ owl:Nothing` ⇒ `C ⊑ owl:Nothing` |
| CR7 | `p rdfs:domain A`, `x p y` ⇒ `x type A` |
| CR8 | `p rdfs:range A`, `x p y` (y an IRI or blank node) ⇒ `y type A` |
| CR9 | `p` reflexive ⇒ `x p x` for every IRI `x` that is the subject of a triple (classes and properties included) |
| CR10 | Three-property chain: `p ← p₁ ∘ p₂ ∘ p₃` ⇒ `x p w` |
| ABox typing | `x type C`, `C ⊑ D` ⇒ `x type D` |
| ABox intersection | `x type A₁`, `x type A₂`, `C ≡ A₁ ⊓ A₂` ⇒ `x type C` |
| ABox existential | `x p y`, `y type B`, `∃p.B ⊑ C` ⇒ `x type C` |
| hasKey | `C hasKey (p)`, `x` and `y` of type `C` share a value of `p` ⇒ `x owl:sameAs y` |

### Known limits

- **Unscoped runs miss chained derivations.** Without a source scope (no
  `dataset` or `source_graphs` in the request), the rules read the unnamed
  default graph, and only the ABox typing and intersection rules also read the
  target graph. A rule whose premise was itself derived then does not fire: a
  subclass chain `A ⊑ B ⊑ C ⊑ D` gets `A ⊑ C` but never `A ⊑ D`. Dataset runs
  are scoped and include the target graph.
- **No consistency check in the API.** `El2Classifier::check_consistency`
  reports an unsatisfiable class as an inconsistency, and `classify()` (and so
  `POST /api/reasoning/materialize`) never calls it.
- **Iteration cap.** The loop stops after 500 rounds without saying so.

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
let reasoner = El2Classifier::new(&store);
let report = reasoner.classify()?;

println!(
    "OWL 2 EL: {} triples in {} iterations ({} ms)",
    report.triples_added, report.iterations, report.elapsed_ms
);
```

Entailed triples are stored in `urn:entailment:owl2-el`.

## Typical Use Cases

- **Taxonomy hierarchies**: multi-level classifications that use intersections
  and existential restrictions.
- **Publishing pipelines**: pre-classify an ontology at ingest time and query the
  materialised hierarchy without a live reasoner.

Large biomedical ontologies (SNOMED CT, the Gene Ontology, NCI Thesaurus) are
written in the EL profile, but this reasoner has not been benchmarked or
checked against them.

## Performance

No benchmark of this reasoner is published. Each round re-runs every rule as a
SPARQL update over the whole scope until nothing new is derived, so the cost
grows with the data and with the depth of the hierarchy.

## Differences from OWL 2 RL

OWL 2 EL focuses on forward classification via completion rules; OWL 2 RL uses a broader set of
~80 SPARQL INSERT rules covering more of the RDF-based OWL semantics.  In practice:

- Use **EL** for taxonomy classification with existential restrictions, within
  the limits above.
- Use **RL** for rule-based reasoning over ABox data that uses OWL 2 RL-expressible axioms.
- **DL** adds a few DL-specific rules on top of RL; it is not a complete
  SROIQ(D) reasoner (see [OWL 2 DL](owl2-dl.md)).
