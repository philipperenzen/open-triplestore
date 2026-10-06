# oxjsonld: remove the dot segments of the whole target path when resolving

Draft of an upstream pull request against `oxigraph/oxigraph` (`lib/oxjsonld`).
Not posted.

## Problem

JSON-LD resolves document-relative IRIs, `@base` values, remote context URLs and
`@import` URLs against a base IRI with RFC 3986 §5.2. oxjsonld calls
`oxiri::Iri::resolve`, which removes the dot segments of the reference's own path
but not:

- those of the **base IRI path** it merges with: `http://a/bb/ccc/./d;p?q` + `g`
  gives `http://a/bb/ccc/./g`, and `../g` gives `http://a/bb/ccc/g` (the `..` pops
  the `.` segment) where RFC 3986 gives `http://a/bb/ccc/g` and `http://a/bb/g`;
- those of a **network-path reference**: `//example.org/../x` gives
  `http://example.org/../x` where RFC 3986 gives `http://example.org/x`.

RFC 3986 §5.2.2 sets `T.path = remove_dot_segments(merge(Base.path, R.path))` for a
relative-path reference and `T.path = remove_dot_segments(R.path)` for a reference
with an authority. W3C json-ld-api `toRdf` tests `0122`, `0123` (the RFC 3986 §5.4
examples against a base with `./` and `../`), `e062` and `e091` fail on this.

## Change

A small `iri` module wraps `Iri::resolve` / `Iri::resolve_unchecked`. When the
reference is a network-path reference, or a relative-path reference merged with a
base path that holds a `.` or `..` segment, it rebuilds the target path with RFC
3986 §5.2.4 `remove_dot_segments`; every other case returns what oxiri returned.
An empty-path reference (`""`, `?y`, `#s`) keeps the base path as written, as RFC
3986 says (the tests expect `http://a/bb/ccc/./d;p?y`). Every resolution in
`context.rs` and `expansion.rs` goes through it. Unit tests cover the RFC
examples.

The cleaner fix is in `oxiri` itself (`IriParser::parse_relative` copies the base
path verbatim, and `parse_relative_slash` / `parse_path_start` parse a
network-path with `REMOVE_DOT_SEGMENTS = false`); this patch keeps the change
inside oxjsonld so it does not change `Iri::resolve` for the Turtle, N-Triples and
RDF/XML parsers.
