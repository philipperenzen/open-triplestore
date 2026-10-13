# oxjsonld: apply the index's type-scoped context to the map context of a type map

Draft of an upstream pull request against `oxigraph/oxigraph` (`lib/oxjsonld`).
Not posted.

## Problem

In a type map (`"@container": "@type"`) each key names the type of the node it
maps to. JSON-LD 1.1 API expansion step 13.8.3.2 processes that type's scoped
context into the *map context* the value is expanded with, through the context
processing algorithm with its defaults: the context propagates into nested
nodes. Step 13.8.3.7.4 then adds the expanded type to the node, after expansion,
so the node does not apply the context again as its own type-scoped
(non-propagating) context.

oxjsonld passed the key on as if the node had `"@type": key` in its body, so the
type's context was applied as a non-propagating type-scoped context: nested
nodes fell back to the context without it. W3C json-ld-api `toRdf` test `c013`
fails (`inner-foo` in a nested node expands with the outer definition of `foo`).

## Change

`IndexContainer` in `expansion.rs`: for a `@type` container and a key other than
`@none`, the key's scoped context (looked up in the map context) is processed
into the map context with `propagate = true`, and the type is handed to the node
already IRI-expanded, so the node's own `@type` handling finds no term definition
and applies no scoped context a second time.
