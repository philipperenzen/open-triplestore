# OWL 2 QL Profile

OWL 2 QL (Query Language) is the OWL 2 profile based on DL-Lite_R. It is designed for
**query answering over large ABoxes** (instance data): every consequence about an individual
follows from a closure of the TBox (schema) that is computed once, and what the TBox says
exists without naming it can still be asked about through the query.

Open Triplestore implements it in three parts:

1. **Closure.** The TBox is read as inclusions between DL-Lite *basic concepts* and *roles*, and
   closed: the class and property hierarchies, negative inclusions, and unsatisfiability.
2. **Materialisation.** Every entailed *ground* atom over the individuals in the data is written
   into an entailment graph, and the data is checked for consistency.
3. **Existential query rewriting.** A query's blank nodes are rewritten so they can match the
   anonymous elements that the TBox's existentials imply.

The OWL 2 QL grade is **Partial** (see [Standards](standards.md)). One gap remains, the datatype
map; see [Limitations](#limitations).

> **Open Triplestore role names:** In the OTS UI and API, class definitions and class axioms are stored with graph role **Model** (the T-Box) and ABox content with graph role **Instances**.  Property definitions and relations (`rdfs:subPropertyOf`, `owl:inverseOf`, `rdfs:domain`/`range`) are the **R-Box** and belong to the **Vocabulary** role, even though OWL groups them with the TBox for reasoning purposes.  The standard OWL 2 terms TBox and ABox are used throughout this document as they are defined in the W3C OWL 2 specification, and the reasoner classifies over the TBox+RBox schema together.

## The TBox as DL-Lite_R

A *basic concept* is a named class `A`, or `∃R`, the individuals with some `R`-value. A *role* is a
property `P` or its inverse `P⁻`, so `∃P⁻` is the set of objects of `P`. The reasoner reads:

| RDF | Read as |
|-----|---------|
| `A rdfs:subClassOf C`, `owl:equivalentClass` | `B ⊑ C` for every OWL 2 QL sub- and superclass expression |
| `P rdfs:domain C` / `P rdfs:range C` | `∃P ⊑ C` / `∃P⁻ ⊑ C` |
| `[ owl:onProperty R ; owl:someValuesFrom owl:Thing ]` | `∃R`, on either side (`R` may be `[ owl:inverseOf P ]`) |
| `[ owl:onProperty R ; owl:someValuesFrom A ]` on the right | `B ⊑ ∃R.A`, a *qualified existential* |
| `[ owl:onProperty U ; owl:someValuesFrom xsd:… ]` | `∃U` for a data property `U` |
| `owl:intersectionOf` on the right | one inclusion per member |
| `owl:complementOf`, `owl:disjointWith`, `owl:AllDisjointClasses`, `⊑ owl:Nothing` | negative inclusion `B ⊑ ¬C` |
| `rdfs:subPropertyOf`, `owl:equivalentProperty`, `owl:inverseOf` | `R ⊑ S`, in both polarities (`R⁻ ⊑ S⁻`) |
| `owl:SymmetricProperty` | `P ⊑ P⁻` |
| `owl:propertyDisjointWith`, `owl:AllDisjointProperties` | negative inclusion `R ⊑ ¬S` |
| `owl:AsymmetricProperty` | `P ⊑ ¬P⁻` |
| `owl:ReflexiveProperty` / `owl:IrreflexiveProperty` | every element has / has no `P`-loop |
| `rdfs:range xsd:…` on a data property | the property's values must be in that datatype |

A qualified existential `B ⊑ ∃R.A` is read with a fresh role `F`: `F ⊑ R`, `∃F⁻ ⊑ A` and
`B ⊑ ∃F`. It says that every `B` has *some* `R`-value in `A`. It does **not** make every subject
of `R` a `B`.

Axioms outside the profile are not used, for example `owl:TransitiveProperty`, functional
properties, `owl:hasKey`, property chains, `owl:sameAs`, unions and cardinalities. Each run lists
them in its report:

```json
{ "regime": "owl2-ql", "triples_added": 12, "ignored_axioms": 2,
  "ignored_sample": [
    { "axiom": "owl:TransitiveProperty", "subject": "http://example.org/ancestorOf",
      "reason": "outside OWL 2 QL" },
    { "axiom": "rdfs:subClassOf", "subject": "http://example.org/Pet",
      "reason": "a class expression outside OWL 2 QL" } ] }
```

`ignored_sample` lists the first 20. Both fields are omitted when nothing was ignored.

## Closure and consistency

The hierarchies are closed in memory. A role inclusion `R ⊑ S` also gives `∃R ⊑ ∃S` and
`∃R⁻ ⊑ ∃S⁻`. Negative inclusions are kept as declared, and the closure then works out which basic
concepts and roles are *unsatisfiable*, meaning empty in every model. It repeats until nothing
changes:

- A basic concept is empty if it is below two disjoint concepts, or below an empty one.
- A role is empty if `∃R` or `∃R⁻` is empty, or the role is below an empty role or two disjoint
  roles.
- An empty role empties both of its `∃` concepts.

The fresh roles carry this through a qualified existential. With `A ⊑ ∃r.B`, `r rdfs:range C`
and `B owl:disjointWith C`, the `r`-value an `A` must have would be both a `B` and a `C`, so `A`
itself is empty. The closure writes `A rdfs:subClassOf owl:Nothing`.

A run is **inconsistent** if any of these hold:

| Rule | Example |
|------|---------|
| `ql-cls-disjoint` | an individual is in two disjoint basic concepts (through any inclusions) |
| `ql-cls-nothing` | an individual is in an unsatisfiable concept |
| `ql-prp-disjoint` | a pair is in two disjoint roles (asymmetric properties included) |
| `ql-prp-nothing` | a pair is in an unsatisfiable role |
| `ql-prp-irp` | an irreflexive property has a loop, or includes a reflexive one |
| `ql-different-from` | `a owl:differentFrom a` |
| `ql-dt-range` | a data-property value lies outside its declared datatype |

Some of these hold whatever the data says, because a model always has at least one element. A
reflexive property that is also irreflexive is an example. An inconsistency fails the run with the
rule's name, and the atoms derived so far stay in the target graph.

## Materialisation

`POST /api/reasoning/materialize` with `"regime": "owl2-ql"` writes this into the target graph
(`urn:entailment:owl2-ql` by default), as does a dataset's `owl2-ql` regime in `materialize` mode
(into `urn:entailment:owl2-ql:<dataset>`):

- The named-class and named-property closure: `A rdfs:subClassOf C` and `P rdfs:subPropertyOf Q`
  for every pair the closure entails, including pairs that only follow through an existential.
  Each unsatisfiable class also gets `A rdfs:subClassOf owl:Nothing`.
- Every entailed class membership `x rdf:type A` of an individual in the data.
- Every entailed property assertion `x P y` between individuals in the data: from sub-properties,
  inverses, symmetry and data sub-properties. Every individual also gets a loop for each
  reflexive property.

Atoms the data already asserts are not written again. The individuals are the subjects and objects
of the data's assertions. A literal is never typed. The ABox is read through the store's quad
index, scanning only the classes and properties the TBox mentions.

## Query answering

Under the [SPARQL 1.1 entailment regime](https://www.w3.org/TR/sparql11-entailment/) for OWL 2,
query **variables** bind only to names. Every answer that binds a variable is therefore a ground
atom, and the materialised graph has it. `/sparql?entailment=owl2-ql` (or a dataset whose regime
is `owl2-ql`, with `entailment_dataset`) adds that graph to the query.

**Blank nodes** in a query are existential: they may also stand for an element no one named. With

```turtle
ex:Parent rdfs:subClassOf [ owl:onProperty ex:hasChild ; owl:someValuesFrom ex:Person ] .
ex:ann a ex:Parent .
```

`ASK { ex:ann ex:hasChild [ a ex:Person ] }` is **true**, because Ann has a child even though it
is unnamed. `ASK { ex:ann ex:hasChild ?c }` is **false**, because no child is named.

On the `owl2-ql` path the query's blank nodes are rewritten before it runs. The rewriter takes each
connected part of a basic graph pattern that contains blank nodes, and considers every choice of
which of its blank nodes are anonymous. For each choice:

1. The anonymous blank nodes group into connected pieces. Each piece must fit in the *anonymous
   part of the canonical model*: the tree of unnamed elements below one named individual (its
   *root*). Every named term the piece touches must therefore be that root, and the rewriter
   equates them.
2. It searches for a placement of the piece in that tree, where every element is created by a
   role from its parent and its classes follow from the TBox. Each placement gives a condition on
   the root, such as "the root is in `∃R`" for the roles the piece starts with. A piece that
   touches no named term needs only "some element of that kind exists".
3. Each condition becomes ground atoms over named individuals: the root is in some named class or
   role under `∃R`.

The alternatives are combined with `UNION`, and the part becomes `SELECT DISTINCT` over its
variables. Each binding of those variables therefore counts once, however many named or anonymous
elements satisfy it. Parts without blank nodes are not touched, and a query without blank nodes
runs exactly as written.

The TBox for the rewriting is read from the graphs the query reads, after the caller's read scope
has been applied, and is cached until the next write. A part with more than 8 blank nodes, a
variable predicate or a variable class is left as written. Its blank nodes still match named
individuals, so the answers stay sound.

### Rewriting without a materialised graph

`QLQueryRewriter::rewrite_query` and `POST /api/reasoning/rewrite` give a stand-alone rewriting
over the asserted data. Every atom is also expanded through the hierarchies: `?x a C` becomes a
`UNION` over every basic concept under `C`, and `?x P ?y` a `UNION` over every role under `P`. This
endpoint reads the TBox only from graphs the caller may read; an admin's reads the unnamed default
graph.

```rust
use open_triplestore::reasoning::owl2_ql::{rewrite_existentials, QLQueryRewriter};

// Materialise (the server does this for the regime).
let report = QLQueryRewriter::new(&store)
    .with_sources(vec!["https://example.org/data".into()])
    .materialize()?;

// Over the data plus the entailment graph: rewrite blank nodes only.
let q = "ASK FROM <https://example.org/data> FROM <urn:entailment:owl2-ql> \
         WHERE { <https://example.org/ann> <https://example.org/hasChild> [] }";
let q = rewrite_existentials(&store, q)?.unwrap_or_else(|| q.to_string());

// Or stand-alone, over the asserted data alone.
let q = QLQueryRewriter::new(&store).rewrite_query(
    "SELECT ?x WHERE { ?x a <https://example.org/Person> }")?;
```

## Configuration

```toml
# Cargo.toml
[features]
owl2-ql = ["rdfs-entailment"]
```

## Comparison with OWL 2 RL

| | OWL 2 QL | OWL 2 RL |
|--|---------|---------|
| Approach | TBox closure + ground materialisation + existential rewriting | Forward-chaining rules |
| Existentials (`C ⊑ ∃P.D`) | Answered through blank nodes | Not in the profile |
| Equality, keys, functional properties | Not in the profile | Yes |
| Update handling | Re-materialise (datasets do so on every write) | Re-materialise |

On ontologies that are in both profiles, the two derive the same ground atoms. A randomized
differential test in `tests/owl2_ql_conformance.rs` checks this.

## Limitations

- **Datatypes.** A data-property value is checked against its declared range by datatype family:
  a string where an integer is required is an inconsistency. A check that needs the literal's
  value is not made yet, for example `-5` against `xsd:nonNegativeInteger`. The same applies to
  `∃U.D` on the left with `D` other than `rdfs:Literal`, and to two data ranges that cannot
  overlap. These checks wait for the OWL 2 datatype map. Oxigraph also stores every
  integer-derived literal as `xsd:integer`, so a declared `xsd:byte` reads back as an integer.
- `owl:Thing` memberships are not materialised. `?x a owl:Thing` matches only asserted ones, but
  an anonymous element is a `Thing`.
- Existential rewriting applies to blank nodes in basic graph patterns. It does not apply to
  property paths, variable predicates or variable classes.
- Inside `GRAPH`, the materialised atoms are only visible if the entailment graph itself is named.
