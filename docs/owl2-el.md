# OWL 2 EL Profile

OWL 2 EL (Existential Language) is a tractable sub-language of OWL 2 designed for large
biomedical ontologies such as SNOMED CT, GO, and NCI Thesaurus.

> **Open Triplestore role names:** TBox content = graph role **Model**; ABox content = graph role **Instances**.

It supports:

- Class intersection (`owl:intersectionOf`)
- Existential restriction (`owl:someValuesFrom`)
- Property chains (`owl:propertyChainAxiom`)
- Reflexive properties
- Nominal individuals (`owl:oneOf`)
- `owl:hasKey`

Reasoning is PTIME-complete (polynomial in the ontology size), making it practical for
ontologies with millions of axioms.

## Completion Rules Implemented

| Rule | Description |
|------|-------------|
| CR1 | If `C ⊑ D` and `x type C` then `x type D` (subClassOf inheritance) |
| CR2 | If `C1 ⊓ C2 ⊑ D` and `x type C1` and `x type C2` then `x type D` |
| CR3 | If `C ⊑ ∃r.D` and `x type C` then there exists `y` with `x r y` and `y type D` |
| CR4 | If `∃r.C ⊑ D` and `x r y` and `y type C` then `x type D` |
| CR5 | Property chain: if `r ∘ s ⊑ t` and `x r y` and `y s z` then `x t z` |
| CR6 | Top class subsumption propagation |
| CR7 | Domain propagation: if `p rdfs:domain A` and `x p y` then `x type A` |
| CR8 | Range propagation: if `p rdfs:range A` and `x p y` then `y type A` |
| CR9 | Reflexivity: if `p type ReflexiveProperty` and `x` appears in any triple then `x p x` |
| CR10 | 3-element property chain (`r ∘ s ∘ t ⊑ u`) |
| hasKey | If two individuals share all key property values, assert `owl:sameAs` |

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
use open_triplestore::reasoning::owl2_el::Owl2ELReasoner;
use open_triplestore::store::TripleStore;

let store = TripleStore::open("./data")?;
let reasoner = Owl2ELReasoner::new(&store);
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
