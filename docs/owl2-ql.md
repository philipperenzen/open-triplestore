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

## Algorithm

The rewriter does the atom-by-atom part of **PerfectRef** (Calvanese et al.,
2007) at the SPARQL AST level (not string-based rewriting). The OWL 2 QL grade
is **Partial** (see [Standards](standards.md) and the limits below).

1. Load the TBox from the store as inclusions between basic concepts (a named
   class `A`, `∃P` = the subjects of `P`, `∃P⁻` = its objects) and between
   roles (a property `P` or its inverse `P⁻`).
2. Parse the incoming SPARQL query to an AST via the `spargebra` crate.
3. Walk the AST and rewrite each Basic Graph Pattern (BGP) triple:
   - `?x rdf:type <C>` → `UNION` branches for every basic concept under `C`:
     its subclasses (`?x rdf:type <A>`), the domain side of every property
     whose domain is under `C` (`?x <P> ?fresh`), and the range side of every
     property whose range is (`?fresh <P> ?x`), through the property
     hierarchy and inverses.
   - `?x <P> ?y` → `UNION` branches for every role under `P`: sub-properties
     (`?x <Q> ?y`), and inverses or sub-properties of an inverse (`?y <Q> ?x`).
   - Each generated existential atom gets its own fresh variable, so two of
     them are never joined.
4. Serialize the rewritten AST back to SPARQL and execute.

An existential on the right of an axiom (`C ⊑ ∃P.D`) is not used: it answers
only an atom whose other end is an unbound, unshared variable, which a
one-atom rewriter cannot tell. It does **not** make every subject of `P` a `C`.

## Supported TBox Axioms

| Axiom | Example |
|-------|---------|
| `rdfs:subClassOf` | `ex:Prof rdfs:subClassOf ex:Employee` |
| `owl:equivalentClass` | `ex:Faculty owl:equivalentClass ex:AcademicStaff` |
| `rdfs:subPropertyOf` | `ex:fatherOf rdfs:subPropertyOf ex:parentOf` |
| `owl:equivalentProperty` | `ex:knows owl:equivalentProperty ex:acquaintedWith` |
| `owl:inverseOf` | `ex:teaches owl:inverseOf ex:taughtBy` |
| `rdfs:domain` | `ex:teaches rdfs:domain ex:Person` |
| `rdfs:range` | `ex:teaches rdfs:range ex:Course` |
| unqualified existential on the left | `[ owl:onProperty ex:teaches ; owl:someValuesFrom owl:Thing ] rdfs:subClassOf ex:Teacher` (also on `[ owl:inverseOf ex:teaches ]`) |

> **Note on graph roles:** the *class* axioms above (`rdfs:subClassOf`, `owl:equivalentClass`) are T-Box terms and live in a **Model** graph; the *property* axioms (`rdfs:subPropertyOf`, `owl:equivalentProperty`, `owl:inverseOf`, `rdfs:domain`, `rdfs:range`) are R-Box terms and live in a **Vocabulary** graph.  OWL groups all of them under "TBox" for reasoning, and the QL rewriter loads them together — the role split is about *where the terms are stored and registered*, not about how the reasoner uses them.

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
`urn:entailment:owl2-ql` (sub-properties entailed through inverses included).

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

- Variable predicates (`?x ?p ?y`) and variable classes (`?x a ?c`) cannot be statically
  rewritten.
- Existentials on the right (`C ⊑ ∃P.D`), negative inclusions (`owl:disjointWith`,
  `owl:propertyDisjointWith`), consistency checks, symmetric and reflexive properties and data
  properties are not used.
- The `owl2-ql` regime of `POST /api/reasoning/materialize` writes only the TBox closure, so
  `?entailment=owl2-ql` adds no inferences about individuals.
- `POST /api/reasoning/rewrite` reads the TBox from the graphs the caller may read (an admin's,
  from the unnamed default graph).
- The rewriter loads the full TBox on each call.  For high-throughput scenarios, cache the
  `QLQueryRewriter` instance across requests.
