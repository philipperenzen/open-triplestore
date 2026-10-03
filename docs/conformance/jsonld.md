# JSON-LD 1.1 — the W3C json-ld-api toRdf / fromRdf run

`tests/w3c_jsonld_api_manifests.rs` runs the `toRdf` and `fromRdf` sections
of the W3C JSON-LD 1.1 API test suite (w3c/json-ld-api, vendored unmodified
in `tests/fixtures/w3c-jsonld-api/`, provenance in its `PROVENANCE.md`)
through the JSON-LD processor every parse and serialisation of this server
uses (oxigraph's `oxjsonld`, built with the `rdf-12` feature as in `full`).

**No score is published.** The two sections are a subset of a W3C test
suite, and W3C's test-suite policy allows no claims of performance on a
subset (https://www.w3.org/copyright/test-suites-licenses/). The run is a
development and regression ratchet: every evaluated entry not listed as a
known failure must pass, every listed one must still fail, and a pass floor
is asserted.

## How entries are evaluated

- **toRdf** — the input is parsed with the base IRI the manifest gives it;
  contexts the tests name by IRI are served from the vendored files by a test
  document loader. A positive evaluation test passes when the dataset is
  isomorphic to the expected N-Quads, a negative one when the parse fails
  (the error code is not compared), a syntax test when the parse succeeds.
- **fromRdf** — the input N-Quads are serialised as JSON-LD and parsed back;
  the result must be isomorphic to the expected document read by the same
  processor. This compares RDF, not JSON-LD object equality: the serialiser
  writes a compacted form where the expected outputs are expanded.
- **Skipped by design** (counted by the runner, never silently): JSON-LD
  1.0-only entries (`specVersion` or `processingMode` `json-ld-1.0`),
  generalized RDF, `rdfDirection`, `expandContext`, and the `useNativeTypes` /
  `useRdfType` serialisation options — options the processor does not offer.

## Known gaps

The runner's `KNOWN_FAILURES` list is the authority; in summary:

- **`@direction` under the `rdf-12` build** (toRdf `di02`, `di04`–`di06`): the
  processor writes an RDF 1.2 directional language string (`"…"@en--ltr`);
  JSON-LD 1.1 drops the direction unless `rdfDirection` is set.
- **Type-scoped contexts in type maps** (toRdf `c013`): the containing term's
  scoped context is applied instead of the type's.
- **An invalid `@base`** (toRdf `li12`) fails the parse instead of leaving
  relative IRIs unresolved.
- **Serialisation** (fromRdf `0016`, `js08`, `js09`): a list whose nodes are
  typed `rdf:List` is written in a form that reads back differently, and an
  invalid `rdf:JSON` literal is serialised instead of refused.
- **Not offered** (skipped by design): `rdfDirection`, `expandContext`,
  `useNativeTypes` / `useRdfType`, generalized RDF and the JSON-LD 1.0
  processing mode.

All of these sit in the JSON-LD processor (`oxjsonld` 0.2.6), not in this
server's code; fixing them means patching it, upstream or in the vendored
Oxigraph fork.

## Remote contexts in the server

The suite's remote contexts come from the test loader. In the server, every
JSON-LD parse resolves a context named by IRI through the document loader in
`src/jsonld/mod.rs`: bundled W3C contexts offline, otherwise only URLs in
`OTS_REMOTE_ALLOWLIST`, with redirects and `Link` alternates followed inside
the allowlist, a size cap and a cache ([formats.md](../formats.md#json-ld-remote-contexts)).
`tests/jsonld_document_loader_http.rs` pins that behaviour.
