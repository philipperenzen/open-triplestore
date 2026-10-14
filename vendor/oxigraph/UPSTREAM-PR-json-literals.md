# Draft upstream pull request (not posted)

Target: `oxigraph/oxigraph`, branch `main`, files `lib/oxigraph/src/sparql/update.rs`
and `lib/oxigraph/src/sparql/error.rs`.

---

**Title:** SPARQL UPDATE: refuse to insert an ill-typed `rdf:JSON` literal

**Problem**

`rdf:JSON` (JSON-LD 1.1 §10.2; the RDF 1.2 JSON datatype) has as its lexical space
the strings that match the JSON grammar (RFC 8259 §2). A literal outside it, such as
`"bareword"^^rdf:JSON` or `"[{]"^^rdf:JSON`, denotes no value, and a JSON-LD
processor must refuse to serialize it (`invalid JSON literal`; W3C json-ld-api
`fromRdf` tests `js08` and `js09`). Once such a literal is in a store, every JSON-LD
download of the graph that holds it fails. `INSERT DATA`, `DELETE/INSERT` and `LOAD`
store it without complaint, so the failure surfaces far from the write that caused it.

**Change**

- `UpdateEvaluationError::IllTypedLiteral(Literal)` (the enum is `#[non_exhaustive]`),
  mapped to `io::ErrorKind::InvalidInput`.
- `check_inserted`: a quad whose object is an `rdf:JSON` literal (or, with `rdf-12`,
  a triple term whose object is one) that is not exactly one JSON value is refused.
  The check uses `json-event-parser`, the parser `oxjsonld` already depends on.
- `eval_insert_data` checks every quad before inserting any; `eval_delete_insert`
  checks every `Insert` mutation before applying the mutations; `eval_load` checks
  each parsed quad as it goes (the transaction is not committed on error). Both
  evaluators (transactional and write-only) do the same.

Other datatypes are not checked: their ill-typed literals have a defined place in
the encoder (`SmallTypedLiteral` / `BigTypedLiteral`) and serialize in every format.
`rdf:JSON` is the one datatype a supported output format cannot write when the
lexical form is wrong.

**Tests**

`INSERT DATA { <s> <p> "[{]"^^rdf:JSON }` and
`INSERT { <s> <p> ?o } WHERE { BIND(STRDT("bareword", rdf:JSON) AS ?o) }` fail with
`IllTypedLiteral` and leave the store empty; `INSERT DATA { <s> <p> "[1]"^^rdf:JSON }`
succeeds.

**Open question for review**

Whether this should be opt-in (`SparqlEvaluator::with_literal_validation`) rather than
the default. A store that never serializes JSON-LD may prefer to keep ill-typed
literals, as RDF 1.1 allows them.
