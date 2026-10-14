# Draft upstream PR: make a configured SparqlParser cheap to clone

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.4` branch.

## Title

spargebra: share the custom aggregate set between parser clones

## Body

`SparqlParser::parse_query` consumes the parser, so an application that
declares custom aggregate functions either rebuilds the parser for every query
(`with_custom_aggregate_function` per IRI) or keeps one and clones it. Both copy
the `HashSet<NamedNode>` of aggregate IRIs — an allocation and a hash per IRI per
query. With six aggregates declared that was about 0.7 µs of a 2.6 µs parse of
`SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }` on an M1 Pro.

### Change

`custom_aggregate_functions` is an `Arc<HashSet<NamedNode>>` in `SparqlParser`
and `ParserState`; `with_custom_aggregate_function` uses `Arc::make_mut`. A clone
of a configured parser is a reference count. No public API changes.

### Tests

Unchanged; parsing behaviour is identical. Measured: a clone of a parser with
six aggregates followed by the parse above takes 2.60 µs, against 3.38 µs for a
parser rebuilt per query and 2.65 µs for a parser with one aggregate before.
