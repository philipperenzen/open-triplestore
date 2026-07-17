# OWL 2 QL Profile

OWL 2 QL (Query Language) is a sub-language of OWL 2 based on DL-Lite.  It is designed for
**query answering over large ABoxes** (instance data) without materialising inferences.  Instead
of pre-computing entailed triples, OWL 2 QL rewrites each incoming SPARQL query to account for
the TBox (schema) axioms at query time.

> **Open Triplestore role names:** In the OTS UI and API, class definitions and class axioms are stored with graph role **Model** (the T-Box) and ABox content with graph role **Instances**.  Property definitions and relations (`rdfs:subPropertyOf`, `owl:inverseOf`, `rdfs:domain`/`range`) are the **R-Box** and belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning purposes.  The standard OWL 2 terms TBox and ABox are used throughout this document as they are defined in the W3C OWL 2 specification, and the reasoner classifies over the TBox+RBox schema together.

This makes it ideal for:
- Read-heavy workloads where the TBox is small and relatively static.
- Scenarios where storage of entailed triples is prohibitive.
- Integration with existing relational databases via SPARQL-to-SQL rewriting.

## Algorithm: PerfectRef

The implementation uses the **PerfectRef** algorithm (Calvanese et al., 2007) at the SPARQL AST
level (not string-based rewriting):

1. Load the TBox from the store (subClassOf, equivalentClass, subPropertyOf, equivalentProperty,
   inverseOf, rdfs:domain).
2. Compute full transitive closure in pure Rust.
3. Parse the incoming SPARQL query to an AST via the `spargebra` crate.
4. Walk the AST and rewrite each Basic Graph Pattern (BGP) triple:
   - `?x rdf:type <C>` → `UNION` branches for all subclasses of `C` (because any individual of
     a subclass of `C` satisfies the query).
   - `?x <P> ?y` → `UNION` branches for all subproperties of `P`, plus inverse alternatives
     where `owl:inverseOf` applies.
   - `rdf:domain` axioms generate existential alternatives: if `P rdfs:domain C` then
     `?x rdf:type C` can be satisfied by `?x <P> ?_`.
5. Serialize the rewritten AST back to SPARQL and execute.

## Supported TBox Axioms

| Axiom | Example |
|-------|---------|
| `rdfs:subClassOf` | `ex:Prof rdfs:subClassOf ex:Employee` |
| `owl:equivalentClass` | `ex:Faculty owl:equivalentClass ex:AcademicStaff` |
| `rdfs:subPropertyOf` | `ex:fatherOf rdfs:subPropertyOf ex:parentOf` |
| `owl:equivalentProperty` | `ex:knows owl:equivalentProperty ex:acquaintedWith` |
| `owl:inverseOf` | `ex:teaches owl:inverseOf ex:taughtBy` |
| `rdfs:domain` | `ex:teaches rdfs:domain ex:Person` |

> **Note on graph roles:** the *class* axioms above (`rdfs:subClassOf`, `owl:equivalentClass`) are T-Box terms and live in a **Model** graph; the *property* axioms (`rdfs:subPropertyOf`, `owl:equivalentProperty`, `owl:inverseOf`, `rdfs:domain`) are R-Box terms and live in a **Vocabulary** graph.  OWL groups all of them under "TBox" for reasoning, and the QL rewriter loads them together — the role split is about *where the terms are stored and registered*, not about how the reasoner uses them.

## Configuration

```toml
# Cargo.toml
[features]
owl2-ql = ["rdfs-entailment"]
```

## API Usage

```rust
use open_triplestore::reasoning::owl2_ql::QLQueryRewriter;
use open_triplestore::store::TripleStore;

let store = TripleStore::open("./data")?;
let rewriter = QLQueryRewriter::new(&store);

// Rewrite a query before executing
let sparql = "SELECT ?x WHERE { ?x rdf:type <http://example.org/Employee> }";
let rewritten = rewriter.rewrite_query(sparql)?;
let results = store.query(&rewritten)?;
```

### TBox Materialisation (optional)

For inspection and debugging, the computed TBox closure can be materialised into a named graph:

```rust
let report = rewriter.materialize_tbox()?;
println!("TBox closure: {} axioms in <{}>", report.triples_added, report.target_graph);
```

This writes the transitively closed `rdfs:subClassOf` and `rdfs:subPropertyOf` hierarchy into
`urn:entailment:owl2-ql`.

## Example

Given this TBox:

```turtle
ex:PhD    rdfs:subClassOf ex:Student .
ex:Student rdfs:subClassOf ex:Person .
ex:fatherOf rdfs:subPropertyOf ex:parentOf .
ex:teaches owl:inverseOf ex:taughtBy .
```

And this ABox:

```turtle
ex:alice rdf:type ex:PhD .
ex:bob ex:fatherOf ex:alice .
ex:carol ex:teaches ex:cs101 .
```

The query `ASK { ex:alice rdf:type ex:Person }` is rewritten to:

```sparql
ASK {
  { ex:alice rdf:type <ex:Person> }
  UNION { ex:alice rdf:type <ex:Student> }
  UNION { ex:alice rdf:type <ex:PhD> }
}
```

...which evaluates to `true` because `ex:alice rdf:type ex:PhD` is in the store.

## Comparison with OWL 2 RL

| | OWL 2 QL | OWL 2 RL |
|--|---------|---------|
| Approach | Query rewriting (no materialisation) | Forward chaining (materialisation) |
| Storage overhead | None | O(inferred triples) |
| Query overhead | Per-query TBox load + rewrite | Zero (inferences already stored) |
| Best for | Read-heavy, small TBox | Write-once, query-many |
| Update handling | Instant (TBox/ABox changes reflect immediately) | Requires re-materialisation |

## Limitations

- Variable predicates (`?x ?p ?y`) cannot be statically rewritten.
- `owl:someValuesFrom` restrictions in the TBox require an existential witness that QL rewriting
  cannot generate; use OWL 2 EL or RL for those axioms.
- The rewriter loads the full TBox on each call.  For high-throughput scenarios, cache the
  `QLQueryRewriter` instance across requests.
