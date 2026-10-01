# OWL 2 QL Profile

OWL 2 QL (Query Language) is a sub-language of OWL 2 based on DL-Lite.  It is designed for
**query answering over large ABoxes** (instance data) without materialising inferences.  Instead
of pre-computing entailed triples, OWL 2 QL rewrites each incoming SPARQL query to account for
the TBox (schema) axioms at query time.

> **Open Triplestore role names:** In the OTS UI and API, class definitions and class axioms are stored with graph role **Model** (the T-Box) and ABox content with graph role **Instances**.  Property definitions and relations (`rdfs:subPropertyOf`, `owl:inverseOf`, `rdfs:domain`/`range`) are the **R-Box** and belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning purposes.  The standard OWL 2 terms TBox and ABox are used throughout this document as they are defined in the W3C OWL 2 specification, and the reasoner classifies over the TBox+RBox schema together.

The rewriter here covers part of the profile; the [Standards](standards.md) page
grades OWL 2 QL **Partial**, and the limits are listed [below](#limitations).
Query rewriting suits read-heavy workloads with a small, fairly static TBox,
where storing entailed triples is not wanted.

## Algorithm

The rewriter expands each triple pattern on its own, over hierarchies closed in
Rust. It is **not** PerfectRef (Calvanese et al., 2007): there is no reduction
step that unifies atoms, and existential restrictions are not rewritten across
atoms.

1. Load the TBox from the store (subClassOf, equivalentClass, subPropertyOf, equivalentProperty,
   inverseOf, rdfs:domain, and `someValuesFrom` restrictions on the right of subClassOf).
2. Compute the transitive closure of the class and property hierarchies.
3. Parse the incoming SPARQL query to an AST via the `spargebra` crate.
4. Walk the AST and rewrite each Basic Graph Pattern (BGP) triple:
   - `?x rdf:type <C>` → `UNION` branches for all subclasses of `C`.
   - `?x <P> ?y` → `UNION` branches for all subproperties of `P`, plus the
     reversed pattern for each property declared `owl:inverseOf` `P`.
   - If `P rdfs:domain C`, then `?x rdf:type C` also matches `?x <P> ?_ql_any`.
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
use std::path::Path;

let store = TripleStore::open(Path::new("./data"))?;
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

- **Unsound existential mapping.** An axiom `C rdfs:subClassOf [ owl:someValuesFrom D ; owl:onProperty P ]`
  is treated like `P rdfs:domain C`. With `ex:Parent ⊑ ∃ex:hasChild.ex:Person`
  and `ex:x ex:hasChild ex:y`, `ASK { ex:x a ex:Parent }` wrongly returns true.
  A fix is planned.
- **One shared fresh variable.** Every domain expansion uses the same variable
  `?_ql_any`, so two expansions in one query are joined on it, and it appears in
  `SELECT *` results.
- **Partial hierarchy handling.** `rdfs:range` is not used. Domain expansion
  applies only to the class named in the query, not to its subclasses, and not
  through sub-properties. Inverses are not combined with sub-properties, and an
  anonymous inverse (`[ owl:inverseOf ex:p ]`) is ignored.
- **Not covered:** negative inclusions and consistency checking,
  symmetric/reflexive properties, data properties, `?x a ?c` with a variable
  class, and variable predicates (`?x ?p ?y`), which cannot be statically
  rewritten.
- **HTTP exposure.** The `owl2-ql` regime of `POST /api/reasoning/materialize`
  and of datasets writes only the TBox closure (see
  [TBox Materialisation](#tbox-materialisation-optional)); `?entailment=owl2-ql`
  on `/sparql` adds that graph to the query, so no query is rewritten and no
  ABox inference is returned. `POST /api/reasoning/rewrite` returns the
  rewritten query; its TBox comes from the unnamed default graph only.
- The rewriter loads the full TBox on each call.  For high-throughput scenarios, cache the
  `QLQueryRewriter` instance across requests.
