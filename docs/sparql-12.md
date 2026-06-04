# SPARQL 1.2 Support

## Overview

SPARQL 1.2 is the in-progress revision of the SPARQL query language, adding
support for RDF 1.2 triple terms (embedded triples / RDF-star), new built-in
functions, and new query forms. This document describes what is implemented,
what is partially implemented, and what is planned.

## Current Status

| Feature | Status | Notes |
|---------|--------|-------|
| Triple terms (RDF-star) | ✅ | `<< s p o >>` syntax, `TRIPLE()`, `SUBJECT()`, `PREDICATE()`, `OBJECT()`, `isTRIPLE()` |
| ADJUST function | ✅ | Timezone and duration arithmetic on `xsd:dateTime` |
| `rdf:triple` / `rdf:subject` etc. | ✅ | Custom function registration under RDF 1.2 IRIs |
| SPARQL Results JSON for triple terms | ✅ | `{"type":"triple","value":{...}}` serialization |
| `LATERAL` joins | 🟡 | Planned — requires engine changes in the opengraph fork |
| `CALL` (service extension) | 🟡 | Planned |
| `COUNT` deduplication changes | 🟡 | Minor spec change, planned |

## Enabling SPARQL 1.2 / RDF-star

Enable via the `rdf-12` feature flag:

```toml
# Cargo.toml
[dependencies]
open-triplestore = { version = "0.1", features = ["rdf-12"] }
```

Or at the binary level (already on if you build with `--features full`).

## Triple Terms (RDF-star)

Triple terms allow triples to appear as the subject or object of other triples,
enabling annotation of statements:

```sparql
PREFIX ex: <http://example.org/>

# Assert a reified triple and annotate it
INSERT DATA {
  << ex:alice ex:knows ex:bob >> ex:confidence "0.95"^^xsd:decimal .
}
```

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

The `ADJUST` function adjusts a `dateTime` or `date` value:

```sparql
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

SELECT (ADJUST(?dt, "+05:00"^^xsd:string) AS ?local) WHERE {
  BIND("2024-01-15T10:00:00Z"^^xsd:dateTime AS ?dt)
}
```

Supported second-argument forms:

- Timezone offset: `"+05:00"`, `"-03:30"`, `"Z"`, `"UTC"`
- `xsd:dayTimeDuration`: `"PT5H"`, `"P1DT2H30M"`, `"-PT30M"`

The function is registered at `<http://www.w3.org/ns/sparql#adjust>`.

## Planned: LATERAL Joins

`LATERAL` joins allow a subquery in the right-hand side to reference variables
bound by the left-hand side:

```sparql
SELECT ?person ?latestEvent WHERE {
  ?person a ex:Person .
  LATERAL {
    SELECT ?latestEvent WHERE {
      ?person ex:hasEvent ?latestEvent .
    }
    ORDER BY DESC(?latestEvent)
    LIMIT 1
  }
}
```

This requires correlated subquery evaluation in the query engine. It is planned
for the opengraph fork (`spareval` modification). Until then, queries using
`LATERAL` will receive a parse error from the upstream Oxigraph parser.

**Workaround:** Express `LATERAL` as a `OPTIONAL { ... }` with `BIND` where
possible, or use aggregation:

```sparql
# Equivalent without LATERAL:
SELECT ?person ?latestEvent WHERE {
  ?person a ex:Person .
  {
    SELECT ?person (MAX(?e) AS ?latestEvent) WHERE {
      ?person ex:hasEvent ?e .
    }
    GROUP BY ?person
  }
}
```

## Configuration

```rust
// Enable RDF-star / SPARQL 1.2 triple terms in the store
let store = TripleStore::open("./data")
    .with_feature(Feature::RdfStar)
    .build()?;
```

When running the HTTP server, RDF-star support is automatically enabled if the
`rdf-12` cargo feature is compiled in. No runtime configuration is needed.

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

This matches the SPARQL 1.2 Working Draft results format extension.

## Conformance Notes

The implementation is based on:
- [SPARQL 1.2 Query Language Working Draft](https://www.w3.org/TR/sparql12-query/)
- [RDF 1.2 Concepts](https://www.w3.org/TR/rdf12-concepts/)
- Oxigraph 0.4 native RDF-star support (via `spargebra` and `spareval`)

Known gaps vs the full SPARQL 1.2 WD:
- `LATERAL` not yet implemented (parser will reject)
- `CALL` not yet implemented
- Annotation syntax (`~`) in Turtle 1.2 parsing depends on Oxigraph RDF 1.2 parser progress
