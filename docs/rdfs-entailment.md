# RDFS Entailment

The `rdfs-entailment` feature materialises RDFS entailment (RDF 1.1 Semantics §8–9) with a
forward-chaining engine: the RDF pattern `rdfD2`, the RDFS patterns `rdfs1`–`rdfs13`, and the
RDF and RDFS axiomatic triples, in one fixed-point loop.

## Patterns implemented

| Pattern | Concludes | Notes |
|---------|-----------|-------|
| rdfD2 | the predicate of every triple is an `rdf:Property` | |
| rdfs1 | each recognized datatype in use is an `rdfs:Datatype` | see [exemptions](#exemptions-decision-d11) |
| rdfs2 | `rdfs:domain`: the subject has the class | |
| rdfs3 | `rdfs:range`: the object has the class | not for a literal object (a literal subject is no RDF triple) |
| rdfs4a / rdfs4b | subjects and non-literal objects are `rdfs:Resource` | |
| rdfs5 / rdfs11 | `rdfs:subPropertyOf` / `rdfs:subClassOf` are transitive | |
| rdfs6 / rdfs10 | properties / classes are their own sub-property / sub-class | |
| rdfs7 / rdfs9 | triples and types follow sub-properties and sub-classes | |
| rdfs8 | classes are sub-classes of `rdfs:Resource` | |
| rdfs12 | container-membership properties are sub-properties of `rdfs:member` | |
| rdfs13 | datatypes are sub-classes of `rdfs:Literal` | |

The axiomatic triples (the domains and ranges of the RDF and RDFS vocabulary, `rdf:nil a
rdf:List`, `rdfs:Datatype rdfs:subClassOf rdfs:Class`, …) are written to the entailment graph
before the loop, so they chain with the patterns: `rdfs:Resource rdfs:subClassOf ex:Thing`
types every resource `ex:Thing`, and a class used only as the object of `rdf:type` is a class
(the range of `rdf:type`), hence a sub-class of `rdfs:Resource`. The patterns that scan every
triple (`rdfD2`, `rdfs4a`, `rdfs4b`) run once the others reach their fixed point, and the loop
goes on while they add anything.

### Exemptions (decision D11)

The RDFS closure of any graph is infinite; the materialiser writes a finite part of it that is
bounded by the data:

- **`rdf:_n`.** The container-membership axioms (`rdf:_n a rdf:Property`, `a
  rdfs:ContainerMembershipProperty`, domain and range `rdfs:Resource`) are written for
  `rdf:_1` … `rdf:_N`, where `N` is the largest index any IRI in scope uses (none when no
  `rdf:_n` is used).
- **`rdfs1`.** The recognized datatypes (`D`) are the OWL 2 datatype map's types and
  `rdf:langString`; the materialiser declares those that literals in scope use, plus
  `xsd:string` and `rdf:langString`, which every RDF 1.1 interpretation recognizes.
- **`rdfD1`** (a typed literal entails a blank node of its datatype) is not materialised: its
  conclusions are existential and no query can tell them from the literal itself.
- Datatype inconsistencies (an ill-typed literal of a recognized datatype) are not reported by
  the RDFS engine; the OWL 2 RL engine reports them (`dt-not-type`).

## Configuration

Enable the feature in `Cargo.toml`:

```toml
[features]
rdfs-entailment = []   # already defined; add to your feature selection
```

Or activate it as part of a compound feature:

```toml
# owl2-rl implies rdfs-entailment
open-triplestore = { features = ["owl2-rl"] }
```

## API Usage

```rust
use open_triplestore::reasoning::common::RDFS_ENTAILMENT_GRAPH;
use open_triplestore::reasoning::rdfs::RdfsMaterializer;
use open_triplestore::store::TripleStore;

let store = TripleStore::open("./data")?;
let report = RdfsMaterializer::with_target(&store, RDFS_ENTAILMENT_GRAPH).materialize()?;

println!(
    "RDFS: {} triples in {} iterations ({} ms)",
    report.triples_added, report.iterations, report.elapsed_ms
);
```

Without `with_sources(graphs)` the patterns read the unnamed default graph and the entailment
graph; with it, only those graphs and the entailment graph. Over HTTP,
`POST /api/reasoning/materialize` with `"regime": "rdfs"` runs it (see
[reasoning.md](reasoning.md)).

## Querying the entailed triples

Entailed triples are stored in the named graph `urn:entailment:rdfs`. Add
`?entailment=rdfs` to a SPARQL request to fold that graph into the query's default graph, or
name it in the query:

```sparql
SELECT * FROM <urn:entailment:rdfs> WHERE { ?s rdf:type ?c }
```

A dataset can select `rdfs` as its regime and keep its own entailment graph up to date
(`PUT /api/datasets/<id>/entailment`, [reasoning.md](reasoning.md#per-dataset-entailment-selectable-regime-materialisation-toggle)).

## Conformance

`tests/rdfs_conformance.rs` tests each pattern, the axiomatic triples and the exemptions.
`tests/w3c_rdf_mt_manifests.rs` runs the W3C RDF 1.1 Semantics test cases and
`tests/w3c_sparql11_entailment_manifests.rs` the RDFS cases of the SPARQL 1.1 entailment-regime
tests; known gaps are in [conformance/entailment.md](conformance/entailment.md). No score is
published (W3C test-suite policy).

## Entailment Graph

Materialised triples are written to `urn:entailment:rdfs`.  This graph can be inspected,
cleared, and rebuilt independently of the asserted data:

```sparql
# Count entailed triples
SELECT (COUNT(*) AS ?n) FROM <urn:entailment:rdfs> WHERE { ?s ?p ?o }

# Clear and rebuild
CLEAR GRAPH <urn:entailment:rdfs>;
-- then call RdfsMaterializer::materialize() again
```

## Performance Notes

- `rdfD2`, `rdfs4a` and `rdfs4b` generate one triple per distinct predicate, subject and
  object, and each scans every triple in scope once per round in which the other patterns
  have reached their fixed point (usually two rounds).
- The fixed-point loop converges in ≤ `log(depth)` iterations for typical hierarchies.
