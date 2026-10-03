# SPARQL 1.2 Support

## Overview

SPARQL 1.2 is the in-progress revision of the SPARQL query language. The
reference here is the [W3C Working Draft of 1 October 2026](https://www.w3.org/TR/sparql12-query/);
its Appendix A lists the changes from SPARQL 1.1. The query engine is
Oxigraph 0.5 (`spargebra` and `spareval`), so most of what follows is its
behaviour, compiled in with the `rdf-12` feature.

## Current Status

The rows follow the normative changes in Appendix A of the Working Draft.

| SPARQL 1.2 change | Status | Notes |
|---------|--------|-------|
| Triple terms, reifiers, reified triples, annotation syntax | ✅ | `<<( s p o )>>` triple terms, `<< s p o >>` reified triples, `~ reifier`, `{\| \|}` annotations — see below |
| `TRIPLE`, `isTRIPLE`, `SUBJECT`, `PREDICATE`, `OBJECT` | ✅ | Native built-ins |
| Literal base direction (`"text"@ar--rtl`) | ✅ | Parsed, stored and returned on every read path (the [columnar copy](performance.md#4-the-columnar-copy-opengraphcolumnar) hands queries over directional literals to the engine) |
| `LANGDIR`, `hasLANG`, `hasLANGDIR`, `STRLANGDIR` | ✅ | Native built-ins |
| `VERSION` declaration | ✅ | Accepted by the parser |
| Duplicate variables in `VALUES` are an error | ✅ | Rejected at parse time |
| `!!` (double negation) | ✅ | Accepted by the parser |
| `ORDER BY` with triple terms, the formal `EXISTS` definition, `sameValue` | ✅ | Oxigraph's evaluation, run against the W3C suite |
| SPARQL Results JSON for triple terms and base direction | ✅ | `{"type":"triple","value":{...}}`; a directional literal carries `"its:dir"` |

The remaining changes in Appendix A (XSD 1.1 and XPath 3.1 references, the
removal of simple literals, escape processing, the algebra rewrites) change how
the specification is written rather than what a query returns.

**Evidence.** The W3C SPARQL 1.2 test suite (`sparql/sparql12` of
w3c/rdf-tests) is vendored unmodified and runs in CI through
`tests/w3c_sparql12_manifests.rs`, every query-evaluation entry on the engine
and again through the in-memory mirror; [conformance/sparql12.md](conformance/sparql12.md)
describes the run and lists the entries that wait on an open W3C Working
Group issue (no score is published: the copy is a subset of a W3C test suite).
`tests/sparql12_conformance.rs` pins, on both read paths, triple-term
semantics (quoting, referential opacity, reifiers, `TRIPLE()`, nested and
per-graph cases), `VERSION`, the `LANGDIR` family and base direction, `~` and
`{| |}` in updates and queries, the rejection of duplicate `VALUES`
variables, and the two SEP extensions below. The RDF 1.2 syntax suites run
through `tests/w3c_rdf12_manifests.rs` ([conformance/rdf12.md](conformance/rdf12.md)).

## SEP extensions (not part of SPARQL 1.2)

Oxigraph also compiles in two extensions from the SPARQL 1.2 Community Group's
SPARQL Enhancement Proposals. They are **not** in the Working Draft, are always
on, and have no switch. Other SPARQL 1.2 engines may not accept them.

| Extension | Proposal | Notes |
|---|---|---|
| `LATERAL` | SEP-0006 | Correlated join: the right-hand side runs once per left-hand solution ([below](#lateral-joins)) |
| `ADJUST(temporal, duration)` | SEP-0002 | Timezone adjustment of `xsd:dateTime`, `xsd:date` and `xsd:time` values ([below](#adjust-function)) |

`CALL` is neither in the Working Draft nor supported: the parser rejects it.

The project also registers a few **non-standard** functions under fixed IRIs:
aliases of the five triple-term built-ins in the `rdf:` namespace
(`rdf:triple`, `rdf:subject`, `rdf:predicate`, `rdf:object`, `rdf:isTriple`)
for older client tooling, and `<http://www.w3.org/ns/sparql#adjust>` (see
below). No W3C specification defines these as functions (`rdf:subject`,
`rdf:predicate` and `rdf:object` are RDF's reification properties).

## Enabling SPARQL 1.2 / RDF 1.2

Enable via the `rdf-12` feature flag:

```toml
# Cargo.toml — the crate is not published (publish = false), so depend on the path
[dependencies]
open-triplestore = { path = "../open-triplestore", features = ["rdf-12"] }
```

Or at the binary level (already on if you build with `--features full`).

## Triple Terms (RDF 1.2)

The engine implements the RDF 1.2 model, not the older RDF-star Community Group
one. A *triple term* `<<( s p o )>>` may appear only as an **object**, and a
statement is annotated through a *reifier* that `rdf:reifies` the triple term.
`<< s p o >>` is shorthand for such a reifier (a fresh blank node): it annotates
the statement **without asserting it**. The `{| … |}` annotation syntax asserts
the triple and annotates it in one go:

```sparql
PREFIX ex:  <http://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

# Annotate the statement without asserting it
INSERT DATA {
  << ex:alice ex:knows ex:bob >> ex:confidence "0.95"^^xsd:decimal .
}

# Assert the statement and annotate it
INSERT DATA {
  ex:alice ex:knows ex:bob {| ex:confidence "0.95"^^xsd:decimal |} .
}
```

Code written for the Community Group model, where a quoted triple could be a
subject, needs updating; [Standards](standards.md) note 1 has the details.

Query triple terms with pattern matching:

```sparql
SELECT ?s ?p ?o ?conf WHERE {
  << ?s ?p ?o >> ex:confidence ?conf .
}
```

Built-in functions (natively handled by the Oxigraph/spargebra engine):

| Function | Description |
|----------|-------------|
| `TRIPLE(?s, ?p, ?o)` | Construct a triple term |
| `SUBJECT(?t)` | Extract the subject of a triple term |
| `PREDICATE(?t)` | Extract the predicate of a triple term |
| `OBJECT(?t)` | Extract the object of a triple term |
| `isTRIPLE(?t)` | Test whether a value is a triple term |

## ADJUST Function

`ADJUST` (SEP-0002) gives an `xsd:dateTime`, `xsd:date` or `xsd:time` value a new
timezone. The second argument must be an `xsd:dayTimeDuration` (or an
`xsd:duration` with no year or month part) between `-PT14H` and `PT14H`:

```sparql
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

SELECT ?local WHERE {
  BIND(ADJUST("2024-01-15T10:00:00Z"^^xsd:dateTime, "PT5H"^^xsd:dayTimeDuration) AS ?local)
}
# ?local = "2024-01-15T15:00:00+05:00"^^xsd:dateTime
```

A value that already has a timezone is converted (the instant stays the same);
a value without one gets the timezone attached
(`"2024-01-15T10:00:00"` with `"-PT3H30M"` gives `"2024-01-15T10:00:00-03:30"`).
Any other second argument — a string such as `"+05:00"` included — leaves the
result unbound.

### `sparql:adjust` (non-standard)

A separate custom function is registered at
`<http://www.w3.org/ns/sparql#adjust>`. It is only reached by calling that IRI;
the `ADJUST` keyword always means the built-in above. It takes the offset as a
plain string and treats a duration as arithmetic:

- `"+05:00"`, `"-03:30"`, `"Z"` or `"UTC"` converts the value to that timezone;
- `"PT5H"`, `"P1DT2H30M"`, `"-PT30M"` **adds** the duration to the value.

```sparql
PREFIX xsd:    <http://www.w3.org/2001/XMLSchema#>
PREFIX sparql: <http://www.w3.org/ns/sparql#>

SELECT ?local WHERE {
  BIND(sparql:adjust("2024-01-15T10:00:00Z"^^xsd:dateTime, "+05:00") AS ?local)
}
```

## LATERAL Joins

`LATERAL` (SEP-0006, not part of SPARQL 1.2) evaluates its right-hand side
once per solution of the left-hand side, with that solution's bindings in
place — so a subquery can take the latest, largest or first *n* of something
per row:

```sparql
SELECT ?person ?latestEvent WHERE {
  ?person a ex:Person .
  LATERAL {
    SELECT ?person ?latestEvent WHERE {
      ?person ex:hasEvent ?latestEvent .
    }
    ORDER BY DESC(?latestEvent)
    LIMIT 1
  }
}
```

Two rules come from SEP-0006:

- A subquery sees an outer variable only if it **projects** it. Above,
  `SELECT ?person ?latestEvent` correlates; `SELECT ?latestEvent` would not —
  its `?person` would be a different variable, and every person would get the
  same overall latest event.
- The right-hand side may read a left-hand variable but not bind it again
  (with `VALUES` or `BIND`); such a query is rejected when it is parsed.

## Configuration

There is no runtime switch and no builder API for it: `rdf-12` is a compile-time
feature (part of `full`, hence of the default build and the published image).
When it is compiled in, `TripleStore::open(path)` and `TripleStore::in_memory()`
parse and evaluate triple terms; when it is not, the parser rejects them.

## SPARQL Results Format Extensions

When a SPARQL SELECT query returns triple terms, the JSON results format
includes them using the extended representation:

```json
{
  "results": {
    "bindings": [{
      "t": {
        "type": "triple",
        "value": {
          "subject":   {"type": "uri",     "value": "http://example.org/alice"},
          "predicate": {"type": "uri",     "value": "http://example.org/knows"},
          "object":    {"type": "uri",     "value": "http://example.org/bob"}
        }
      }
    }]
  }
}
```

A literal with a base direction carries it as `"its:dir"` next to
`"xml:lang"` (`{"type":"literal","value":"مرحبا","xml:lang":"ar","its:dir":"rtl"}`),
both in `/sparql` results and in the JSON the browse endpoints (triples,
suggestions, blank-node views) return. This matches the SPARQL 1.2 Query
Results JSON Format Working Draft.

## Conformance Notes

The implementation is based on:
- [SPARQL 1.2 Query Language Working Draft](https://www.w3.org/TR/sparql12-query/) (1 October 2026)
- [RDF 1.2 Concepts](https://www.w3.org/TR/rdf12-concepts/)
- Oxigraph 0.5 native RDF 1.2 / triple-term support (via `spargebra` and `spareval`)

Turtle 1.2 and TriG 1.2 input, including `~` reifiers and `{| |}` annotations,
is parsed by Oxigraph's `oxttl`. The grade and the remaining gaps are on the
[Standards](standards.md) page.
