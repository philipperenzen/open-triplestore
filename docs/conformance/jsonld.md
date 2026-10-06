# JSON-LD 1.1 — the W3C json-ld-api toRdf / fromRdf run

`tests/w3c_jsonld_api_manifests.rs` runs the `toRdf` and `fromRdf` sections
of the W3C JSON-LD 1.1 API test suite (w3c/json-ld-api, vendored unmodified
in `tests/fixtures/w3c-jsonld-api/`, provenance in its `PROVENANCE.md`)
through the JSON-LD processor every parse and serialisation of this server
uses: oxigraph's `oxjsonld` 0.2.6, built with the `rdf-12` feature as in
`full` and vendored with fixes in `vendor/oxjsonld/`
([vendor/README.md](../../vendor/README.md#oxjsonld)).

**No score is published.** The two sections are a subset of a W3C test
suite, and W3C's test-suite policy allows no claims of performance on a
subset (https://www.w3.org/copyright/test-suites-licenses/). The run is a
development and regression ratchet: every evaluated entry not listed as a
known failure must pass, every listed one must still fail, and a pass floor
is asserted.

## How entries are evaluated

- **toRdf** — the input is parsed with the base IRI the manifest gives it
  and the entry's `rdfDirection` option; contexts the tests name by IRI are
  served from the vendored files by a test document loader. The runner calls
  `oxjsonld::JsonLdParser` directly, because oxigraph's `RdfParser` has no
  `rdfDirection` setting; an entry without the option gets JSON-LD 1.1's
  `null` (see [`@direction` in the server](#direction-in-the-server)). A
  positive evaluation test passes when the dataset is isomorphic to the
  expected N-Quads, a negative one when the parse fails (the error code is
  not compared), a syntax test when the parse succeeds.
- **fromRdf** — the input N-Quads are serialised as JSON-LD through
  `RdfSerializer`, as the server does, and parsed back; the result must be
  isomorphic to the expected document read by the same processor, both read
  with the entry's `rdfDirection` option. This compares RDF, not JSON-LD
  object equality: the serialiser writes every quad as it is, where the
  expected outputs are expanded and fold lists into `@list`, JSON literals
  into `@json` and direction encodings into `@direction`.
- **Skipped by design** (counted by the runner, never silently): JSON-LD
  1.0-only entries (`specVersion` or `processingMode` `json-ld-1.0`),
  generalized RDF, `expandContext`, and the `useNativeTypes` /
  `useRdfType` serialisation options — options the processor does not offer.
- A manifest input or expected output missing from the vendored files fails
  the run. (One `toRdf` entry, `er56`, names an input from the `expand`
  section; that file is vendored too.)

## Known gaps

The runner's `KNOWN_FAILURES` list is the authority. It holds three `fromRdf`
entries, all where the JSON-LD 1.1 "Serialize RDF as JSON-LD" algorithm
would lose or refuse data the store holds, and the serialiser keeps every
quad as it is instead:

- **`0016`** — a list whose nodes also carry `rdf:type rdf:List`. The
  algorithm folds the list into `@list` and drops those `rdf:type` quads;
  the serialiser writes the list nodes as nodes, so the download reads back
  as the stored graph, type quads included, not as the expected output.
- **`js08`, `js09`** — an `rdf:JSON` literal whose lexical form is not JSON
  (`"bareword"`, `"[{]"`). The algorithm aborts the serialisation (`invalid
  JSON literal`); the serialiser writes it as a typed literal
  (`{"@value": "bareword", "@type": "…#JSON"}`), which reads back as the
  same literal. The store accepts such a literal like any other ill-typed
  one ([datatypes.md](../datatypes.md#other--custom-datatypes)), and
  refusing it would fail the JSON-LD download of the whole graph.

Passing either would mean a JSON-LD download that differs from the Turtle or
N-Quads download of the same graph, so JSON-LD 1.1 stays graded *Partial*
([standards.md](../standards.md), footnote 13).

Fixed in the vendored processor on 2026-10-03, one commit and one upstream
draft each (`vendor/oxjsonld/UPSTREAM-PR-*.md`, not posted):

- IRI resolution removes the dot segments of the whole target path, those of
  a base IRI and of a network-path reference included (RFC 3986 §5.2.2;
  `toRdf` `0122`, `0123`, `e062`, `e091`).
- An `@base` in absolute form that is not a valid IRI is the base, and IRIs
  resolved against it are left out as not well-formed, instead of refusing
  the document (`li12`).
- A type map applies the type's scoped context to the map context, so it
  propagates into nested nodes (`c013`).
- The `rdfDirection` option: `null`, `i18n-datatype` and `compound-literal`
  (`di02`, `di04`–`di06`, and the non-normative `di09`–`di12` and `fromRdf`
  `di05`–`di12`, which the runner used to skip).

## `@direction` in the server

Every JSON-LD parse in the server (uploads, imports, the Graph Store, LDP,
seed bundles, LDES pages) keeps a string's `@direction` as an RDF 1.2
directional language-tagged string (`"…"@ar--rtl`), the processor's default
in the `rdf-12` build; a JSON-LD 1.1 processor without `rdfDirection` drops
the direction (`"…"@ar`). The `jsonld::` unit tests pin this. A JSON-LD
download writes such a literal back with `@language` and `@direction`.

## Remote contexts in the server

The suite's remote contexts come from the test loader. In the server, every
JSON-LD parse resolves a context named by IRI through the document loader in
`src/jsonld/mod.rs`: bundled W3C contexts offline, otherwise only URLs in
`OTS_REMOTE_ALLOWLIST`, with redirects and `Link` alternates followed inside
the allowlist, a size cap and a cache ([formats.md](../formats.md#json-ld-remote-contexts)).
`tests/jsonld_document_loader_http.rs` pins that behaviour.
