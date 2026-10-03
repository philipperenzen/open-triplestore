# oxjsonld: the `rdfDirection` option (`i18n-datatype`, `compound-literal`, null)

Draft of an upstream pull request against `oxigraph/oxigraph` (`lib/oxjsonld`).
Not posted.

## Problem

With the `rdf-12` feature, oxjsonld turns every string with a language and an
`@direction` into an RDF 1.2 directional language-tagged string
(`"abc"@ar--rtl`); without it the direction is dropped. There is no way to ask
for the JSON-LD 1.1 API behaviours: the
[`rdfDirection`](https://www.w3.org/TR/json-ld11-api/#dom-jsonldoptions-rdfdirection)
option's `null` (drop the direction, the default of a JSON-LD 1.1 processor),
`i18n-datatype` (`"abc"^^<https://www.w3.org/ns/i18n#ar_rtl>`) and
`compound-literal` (a blank node with `rdf:value`, `rdf:language`,
`rdf:direction`). W3C json-ld-api `toRdf` tests `di02`, `di04`–`di06` (no
option: direction dropped) fail in an `rdf-12` build, and `di09`–`di12` (the two
non-normative options) cannot be run.

## Change

A `JsonLdRdfDirection` enum and `JsonLdParser::with_rdf_direction`:

- `Ignore` — `rdfDirection` null;
- `I18nDatatype` — the language lowercased, `_`, the direction, appended to
  `https://www.w3.org/ns/i18n#` (a string without a language gives `#_rtl`);
- `CompoundLiteral` — a fresh blank node as the object, with `rdf:value`, the
  lowercased `rdf:language` when there is one, and `rdf:direction`;
- `DirectionalLanguageTaggedString` (`rdf-12` only) — today's behaviour.

The default does not change: `DirectionalLanguageTaggedString` with `rdf-12`,
`Ignore` without it, so existing users see the same quads. The `#[non_exhaustive]`
enum leaves room for what JSON-LD 1.2 settles on for RDF 1.2.

The serialiser is untouched: it writes an `i18n` datatype or a compound-literal
node as the quads they are, which read back as the same quads under the matching
option (the json-ld-api `fromRdf` `di05`, `di06`, `di11`, `di12` round trips).
