# RDFS Entailment

The `rdfs-entailment` feature materialises RDFS entailment (RDF 1.1 Semantics §8–9) with a
forward-chaining engine: the RDF pattern `rdfD2`, the RDFS patterns `rdfs1`–`rdfs13`, and the
RDF and RDFS axiomatic triples, in one fixed-point loop.

## Patterns implemented

| Pattern | Concludes | Notes |
|---------|-----------|-------|
| rdfD2 | the predicate of every triple is an `rdf:Property` | |
| rdfD1 | a typed literal's value is a resource of its datatype | opt-in, see [rdfD1](#rdfd1-literal-values-as-resources) |
| rdfs1 | each recognized datatype in use is an `rdfs:Datatype` | see [the bounded part](#the-bounded-part-of-the-closure-decision-d11) |
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

### The bounded part of the closure (decision D11)

The RDFS closure of any graph is infinite; the materialiser writes a finite part of it that is
bounded by the data:

- **`rdf:_n`.** The container-membership axioms (`rdf:_n a rdf:Property`, `a
  rdfs:ContainerMembershipProperty`, domain and range `rdfs:Resource`) are written for
  `rdf:_1` … `rdf:_N`, where `N` is the largest index any IRI in scope uses (none when no
  `rdf:_n` is used).
- **`rdfs1`.** The recognized datatypes (`D`) are the OWL 2 datatype map's types and
  `rdf:langString`; the materialiser declares those that literals in scope use, plus
  `xsd:string` and `rdf:langString`, which every RDF 1.1 interpretation recognizes.

### rdfD1: literal values as resources

`rdfD1` concludes `ex:a ex:p _:v . _:v rdf:type xsd:integer` from `ex:a ex:p "42"^^xsd:integer`:
a blank node standing for the literal's value, typed with its datatype, which the other patterns
then reason about like any resource (`rdfs3` gives it the property's range, `rdfs9` the
superclasses, `rdfs13` makes it an `rdfs:Literal`). It is **off by default**, because it writes
about two triples per literal-valued triple, every one an existential restatement of the data.
Turn it on per run: `RdfsMaterializer::with_rdfd1(true)`, or `"rdfd1": true` in the body of
`POST /api/reasoning/materialize` with `"regime": "rdfs"`. The materialiser writes one blank node
per data value of a recognized datatype, labelled with a digest of the value: equal values
written differently share it, and a re-run writes the same node.

### Equal values written differently (D-entailment)

The store keeps every literal exactly as written, so `"010"^^xsd:integer`, `"10"^^xsd:integer`
and `"10.0"^^xsd:decimal` are three terms. They denote one value, and under D-entailment a graph
with any of them entails the triple with the others. No materialiser can write every lexical form
of a value, so the regimes match by value at query time: under `?entailment=` (every regime, and
`entailment_dataset=` with a regime), a literal constant in a triple pattern whose datatype is in
the datatype map matches every literal of the same value. Value equality is the XSD 1.1 one of
the OWL 2 datatype map: `"1"^^xsd:integer` and `"1.0"^^xsd:decimal` match, `"1E400"^^xsd:double`
and `"INF"^^xsd:double` match, `"1"^^xsd:integer` and `"1"^^xsd:double` do not (disjoint value
spaces, although SPARQL `=` calls them equal), and `+0` and `-0` do not. A string or boolean
constant becomes a `VALUES` table of its forms (the lookup stays indexed); any other value a
`FILTER` with the server's `sameValue` function, which scans the triple pattern's other matches.
Language-tagged strings and literals of other datatypes match as terms.

### Recognized datatypes

The recognized datatypes (`D`) are the OWL 2 datatype map's types and `rdf:langString`.
`RdfsMaterializer::with_recognized_datatypes` narrows them for a run (`xsd:string` and
`rdf:langString` stay recognized, as in every RDF 1.1 interpretation): a literal of an
unrecognized datatype is then neither checked nor given an `rdfD1` node. Lexical forms are XSD
1.1's lexical spaces with no whitespace normalization (RDF 1.1 Concepts §5.3): `" 3 "^^xsd:int`
is ill-typed. `rdf:XMLLiteral` holds well-balanced, namespace-well-formed XML content: `"<"`
is ill-typed.

### Datatype clashes

A graph with an ill-typed literal of a recognized datatype (`"ten"^^xsd:integer`), or with a
literal whose property's range (or a superclass of it) is a recognized datatype that does not
hold the literal's value (`ex:n rdfs:range xsd:integer . ex:a ex:n "ten"`), has no RDFS
interpretation. The run then ends in an inconsistency (`ill-typed-literal`,
`datatype-clash`; HTTP 422 from `POST /api/reasoning/materialize`); the triples derived up to
that point stay in the entailment graph.

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
use std::path::Path;

let store = TripleStore::open(Path::new("./data"))?;
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

`tests/rdfs_conformance.rs` tests each pattern, the axiomatic triples and the bounded part of
the closure. `tests/w3c_rdf_mt_manifests.rs` runs the W3C RDF 1.1 Semantics test cases (with
each case's recognized datatypes, `rdfD1` on and literals matched by value) and
`tests/w3c_sparql11_entailment_manifests.rs` the RDFS cases of the SPARQL 1.1 entailment-regime
tests; known gaps are in [conformance/entailment.md](conformance/entailment.md). No score is
published (W3C test-suite policy).

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

- `rdfD2`, `rdfs4a` and `rdfs4b` generate one triple per distinct predicate, subject and
  object, and each scans every triple in scope once per round in which the other patterns
  have reached their fixed point (usually two rounds).
- No benchmark of the materialiser is published. Each round re-runs every pattern over the
  whole scope, so the number of rounds grows with the depth of the class and property
  hierarchies.
