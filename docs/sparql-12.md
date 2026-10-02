# SPARQL 1.2 Support

## Overview

SPARQL 1.2 is the in-progress revision of the SPARQL query language, adding
support for RDF 1.2 triple terms (embedded triples / RDF-star), new built-in
functions, and new query forms. This document describes what is implemented,
what is partially implemented, and what is planned.

## Current Status

| Feature | Status | Notes |
|---------|--------|-------|
| Triple terms (RDF 1.2) | ✅ | `<<( s p o )>>` triple terms, `<< s p o >>` reifiers, `{\| \|}` annotations, `TRIPLE()`, `SUBJECT()`, `PREDICATE()`, `OBJECT()`, `isTRIPLE()` |
| ADJUST function | ✅ | Timezone and duration arithmetic on `xsd:dateTime` |
| `rdf:triple` / `rdf:subject` etc. | ✅ | Custom function registration under RDF 1.2 IRIs |
| SPARQL Results JSON for triple terms | ✅ | `{"type":"triple","value":{...}}` serialization |
| `LATERAL` joins | ✅ | The right-hand side sees each left-hand solution's bindings (see below) |
| `CALL` (service extension) | 🟡 | Planned |
| `COUNT` deduplication changes | 🟡 | Minor spec change, planned |

## Enabling SPARQL 1.2 / RDF-star

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

## LATERAL Joins

`LATERAL` evaluates its right-hand side once per solution of the left-hand
side, with that solution's bindings in place — so a subquery can take the
latest, largest or first *n* of something per row:

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

Two rules come from SEP-0006, which the engine follows:

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

This matches the SPARQL 1.2 Working Draft results format extension.

## Conformance Notes

The implementation is based on:
- [SPARQL 1.2 Query Language Working Draft](https://www.w3.org/TR/sparql12-query/)
- [RDF 1.2 Concepts](https://www.w3.org/TR/rdf12-concepts/)
- Oxigraph 0.5 native RDF 1.2 / triple-term support (via `spargebra` and `spareval`)

Known gaps vs the full SPARQL 1.2 WD:
- `CALL` not yet implemented
- Annotation syntax (`~`) in Turtle 1.2 parsing depends on Oxigraph RDF 1.2 parser progress
