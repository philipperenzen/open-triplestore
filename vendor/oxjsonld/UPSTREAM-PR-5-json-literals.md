# oxjsonld: serialize `rdf:JSON` literals as `@json` values, and refuse invalid ones

Draft of an upstream pull request against `oxigraph/oxigraph` (`lib/oxjsonld`).
Not posted.

## Problem

`JsonLdSerializer` writes an `rdf:JSON` literal like any other typed literal:

```json
{"@type": "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON", "@value": "[{]"}
```

The JSON-LD 1.1 API's RDF to Object Conversion (§8.5.2, step 2.4) turns an
`rdf:JSON` literal into its JSON value with `@type: @json`, and when the
lexical form is not JSON, "an invalid JSON literal error has been detected and
processing is aborted". Two consequences:

- W3C json-ld-api `fromRdf` tests `js08` (`"bareword"^^rdf:JSON`) and `js09`
  (`"[{]"^^rdf:JSON`), both `NegativeEvaluationTest` expecting
  `invalid JSON literal`, fail: the serializer writes the document.
- A consumer that reads JSON-LD as JSON (not through an RDF parser) gets a
  string where it expects the JSON value.

## Change

`InnerJsonLdWriter::serialize_term`, for a literal whose datatype is `rdf:JSON`:

- the lexical form is parsed with `json_event_parser::SliceJsonParser`; when it
  is not exactly one JSON value (RFC 8259 §2, whitespace around it allowed),
  `serialize_quad` (or `finish`, see the list-conversion draft) returns an
  `io::Error` of kind `InvalidData` whose message starts `invalid JSON literal`;
- when the lexical form is the canonical form of its value (JCS, RFC 8785 — the
  form the JSON-LD to RDF algorithm writes, §8.6.1 step 6.1.1, and which the
  parser already produces with `serialize_canonical_json`), the value is
  written as `{"@type": "@json", "@value": <the JSON value>}`;
- otherwise (`"{ \"a\" : 1 }"^^rdf:JSON`, `"1.0"^^rdf:JSON`), the literal is
  written as before, typed `rdf:JSON` with its lexical form as a string. Both
  JSON-LD forms denote JSON values, but `@json` reads back as the canonical
  lexical form, which is a different RDF literal: writing it would make a
  serialize / parse round trip change the data. The typed form reads back as
  the exact literal.

`serialize_canonical_json` in `to_rdf.rs` becomes `pub(crate)` so the serializer
can compare against it.

## Tests

W3C json-ld-api `fromRdf` `js01`–`js11` (positive ones round trip; `js08` and
`js09` now error). Unit tests: a canonical object and array come out as `@json`
with the value inline; `"{ \"a\" : 1 }"^^rdf:JSON` round-trips to the same
literal; `"bareword"^^rdf:JSON` errors with `invalid JSON literal`.

## Compatibility

Output for valid canonical `rdf:JSON` literals changes form (same RDF). Output
for an invalid one is now an error instead of a document; a caller that
serializes data it did not validate should be ready for it (the store this fork
lives in refuses such literals on ingest, so it never reaches the serializer).
