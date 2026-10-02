# RDFS Entailment

The `rdfs-entailment` feature enables RDFS entailment via a forward-chaining materialiser that
runs the rdfs1–rdfs13 patterns of the W3C RDF 1.1 Semantics as SPARQL `INSERT` rules. A few
parts of the specification are not covered; see [Gaps](#gaps).

## Rules Implemented

| Rule | Description | Type |
|------|-------------|------|
| rdfs1 | The datatype of every literal in the data is a subclass of `rdfs:Literal` (RDF 1.1's rdfs1 types it `rdfs:Datatype` instead) | Axiomatic |
| rdfs2 | `?x rdf:type ?a` if `?p rdfs:domain ?a` and `?x ?p ?y` | Chained |
| rdfs3 | `?y rdf:type ?a` if `?p rdfs:range ?a` and `?x ?p ?y` | Chained |
| rdfs4a | Every subject → `rdf:type rdfs:Resource` | Axiomatic |
| rdfs4b | Every IRI/bnode object → `rdf:type rdfs:Resource` | Axiomatic |
| rdfs5 | `rdfs:subPropertyOf` transitivity | Chained |
| rdfs6 | Every property → `rdfs:subPropertyOf` itself | Axiomatic |
| rdfs7 | Property inheritance through `rdfs:subPropertyOf` | Chained |
| rdfs8 | Every class → `rdfs:subClassOf rdfs:Resource` | Axiomatic |
| rdfs9 | Type inheritance through `rdfs:subClassOf` | Chained |
| rdfs10 | Every class → `rdfs:subClassOf` itself | Axiomatic |
| rdfs11 | `rdfs:subClassOf` transitivity | Chained |
| rdfs12 | Every `rdfs:ContainerMembershipProperty` → `rdfs:subPropertyOf rdfs:member` | Chained |
| rdfs13 | Every `rdfs:Datatype` → `rdfs:subClassOf rdfs:Literal` | Chained |

**Axiomatic** rules run once after the fixed-point loop, so their output does not feed the
chained rules (`rdfs:Resource rdfs:subClassOf ex:X`, say, never types anything as `ex:X`).
**Chained** rules run inside the loop until no new triples are derived.

### Gaps

- `rdfD2` (`?p` of every triple is an `rdf:Property`) is not run.
- RDF 1.1's `rdfs1` (`?dt rdf:type rdfs:Datatype` for each recognised datatype) is replaced by
  the subclass form above.
- The RDF and RDFS axiomatic triples (`rdf:type rdf:type rdf:Property`, `rdfs:domain rdfs:domain
  rdf:Property`, …) and the `rdf:_n` membership properties are not added.

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
use open_triplestore::reasoning::rdfs::RdfsMaterializer;
use open_triplestore::store::TripleStore;
use std::path::Path;

let store = TripleStore::open(Path::new("./data"))?;
let materialiser = RdfsMaterializer::with_target(&store, "urn:entailment:rdfs");
let report = materialiser.materialize()?;

println!(
    "RDFS: {} triples in {} iterations ({} ms)",
    report.triples_added, report.iterations, report.elapsed_ms
);
```

Entailed triples are stored in the named graph `urn:entailment:rdfs`.  To include them in
query answers, add the graph to the dataset in your SPARQL query:

```sparql
SELECT * FROM <urn:entailment:rdfs> WHERE { ?s rdf:type ?c }
```

## SPARQL Endpoint

`/sparql` adds the entailment graph to a query when the request sets the `entailment` query
parameter. There is no header for it and no server-wide setting:

```http
POST /sparql?entailment=rdfs HTTP/1.1
Content-Type: application/sparql-query

SELECT * WHERE { ?s rdf:type ?c }
```

The graph has to be materialised first (`POST /api/reasoning/materialize` with
`{"regime": "rdfs"}`). A dataset with an entailment regime keeps its own graph; query it with
`entailment_dataset=<id>` instead (see [Reasoning](reasoning.md)).

## Entailment Graph

Materialised triples are written to `urn:entailment:rdfs`.  This graph can be inspected,
cleared, and rebuilt independently of the asserted data:

```sparql
# Count entailed triples
SELECT (COUNT(*) AS ?n) FROM <urn:entailment:rdfs> WHERE { ?s ?p ?o }

# Clear, then call RdfsMaterializer::materialize() again to rebuild
CLEAR GRAPH <urn:entailment:rdfs>
```

## Performance Notes

- Axiomatic rules (rdfs4a, rdfs4b, rdfs6, rdfs8, rdfs10) add about one triple per distinct
  resource, property or class in the data.
- No benchmark of the materialiser is published. Each round re-runs every chained rule over the
  whole scope, so the number of rounds grows with the depth of the class and property
  hierarchies.
