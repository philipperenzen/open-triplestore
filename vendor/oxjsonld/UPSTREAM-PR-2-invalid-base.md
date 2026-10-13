# oxjsonld: take an `@base` in absolute form even when it is not a valid IRI

Draft of an upstream pull request against `oxigraph/oxigraph` (`lib/oxjsonld`).
Not posted.

## Problem

W3C json-ld-api `toRdf` test `li12` ("list elements expanded to IRIs with a bad
`@base`") sets `"@base": "http://invalid/<>/"` and expects the document to
convert: the list element `"test"` (an `@id`) resolves against that base to an IRI
that is not well-formed, so the Deserialize JSON-LD to RDF algorithm leaves out the
`rdf:first` quad and keeps the list node. oxjsonld refuses the whole document with
`invalid base IRI` instead, because `Iri::parse` rejects `<` and `>`.

## Change

In the context processing algorithm (step 5.7), a string `@base` that does not
validate is still taken as the base IRI when it is in absolute form (it starts
with a scheme, RFC 3986 §3.1), unvalidated, as the reference processors do. IRIs
resolved against it are checked where quads are made, as before, so nothing
that is not well-formed reaches the output. A relative `@base` that does not
resolve, and a non-string `@base` (test `er07`), still raise `invalid base IRI`.
