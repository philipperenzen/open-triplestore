# Draft upstream PR: braces outside a quantifier in REGEX patterns

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

spareval: read `{` and `}` outside a quantifier as ordinary characters in REGEX

## Body

SPARQL 1.1 §17.4.3.14 defines `REGEX` with XPath `fn:matches`, whose syntax
(XPath and XQuery Functions and Operators §7.6.1) is the XML Schema 1.0
regular-expression language plus anchors, reluctant quantifiers and
back-references. In XML Schema 1.0 (Part 2, Appendix F) `{` and `}` are
`Char`s, ordinary characters, wherever they do not make a quantifier
`{n}`, `{n,}` or `{n,m}`. The `regex` crate refuses an unescaped `{` that
starts no counted repetition, so `REGEX("{x}", "^({)(.*)(})$")` raises an
error instead of returning true, and `FILTER` drops the row.

Shapes in the wild rely on it: the OGC GeoSPARQL 1.1 validator's
`S18-geojson-content` shape (`sh:pattern "^\\s*$|^\\s*({)(.*)(})\\s*$"`) checks
that a GeoJSON literal is a braced object.

### Change

`compile_pattern` escapes every `{` that does not open a valid quantity after
a quantifiable atom, and every `}` outside one, before it builds the regex.
Character classes and `\p{…}` escapes are copied unchanged.

### Tests

`REGEX("{x}", "^({)(.*)(})$")` is true, `REGEX("aa", "^a{2}$")` still true,
`REGEX("a{2}", "^a\\{2\\}$")` true.
