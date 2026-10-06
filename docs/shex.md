# ShEx Guide

[Shape Expressions](http://shex.io/shex-semantics/) (ShEx 2.1) describe the
shape of RDF nodes: which properties a node has, how many, and what their
values look like. Where SHACL is a vocabulary of constraints, a ShEx schema is
a grammar for a node's neighbourhood. This guide covers what the triplestore
implements, how to call it, and where it stops.

ShEx is in the `full` build (Cargo feature `shex`).

---

## Validating

Two endpoints, both authenticated:

| Endpoint | Reads |
|---|---|
| `POST /api/shex/validate` | every graph the caller may read through `/sparql` (an admin: the whole store) |
| `POST /api/datasets/:id/shex/validate` | the dataset's graphs the caller may read, and nothing else (stored SHACL report graphs excluded) |

```bash
curl -X POST http://localhost:7878/api/shex/validate \
     -H 'Authorization: Bearer <token>' \
     -H 'Content-Type: application/json' \
     -d '{
       "schema": "PREFIX ex: <http://example.org/>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\nex:Person { ex:name xsd:string ; ex:knows @ex:Person * }",
       "shape_map": "ex:alice@ex:Person, {FOCUS a ex:Employee}@ex:Person"
     }'
```

```json
{
  "conforms": false,
  "results": [
    { "focus_node": "http://example.org/alice", "shape": "http://example.org/Person", "status": "Conformant" },
    { "focus_node": "http://example.org/bob", "shape": "http://example.org/Person",
      "status": { "NonConformant": "http://example.org/bob does not conform to <http://example.org/Person>: expected at least 1 <http://example.org/name> matching its value expression, found 0" } }
  ]
}
```

The request body:

| Field | Meaning |
|---|---|
| `schema` | The schema: ShExC text, ShExJ text, or a ShExJ JSON object. |
| `schema_format` | `shexc` or `shexj`. Absent: a JSON object (or text starting with `{`) is ShExJ, anything else ShExC. |
| `base` | Base IRI for relative IRIs in the schema and the shape map. Without one, a relative IRI is an error. |
| `shape_map` | Which nodes to check against which shapes — see below. |

A schema that does not parse, or breaks a schema requirement (an undefined
reference, a label declared twice, a cycle through negation), is a `400` with
the reason; it never validates as "conforms".

### Shape maps

`shape_map` takes three forms:

- **The ShapeMap language** (a string), as in the
  [ShapeMap specification](https://shexspec.github.io/shape-map/):
  `node@shape` associations separated by commas. A node is an IRI, prefixed
  name, blank node or literal; `{FOCUS p o}` / `{FOCUS p _}` selects the
  subjects of matching triples and `{s p FOCUS}` / `{_ p FOCUS}` their
  objects; `@START` names the schema's `start` shape. Prefixed names use the
  schema's prefixes.
  `SPARQL "…"` selectors are not supported.
- **ShapeMap JSON**: `[{"node": "http://example.org/alice", "shape": "http://example.org/Person"}]`.
- **A map from shape to nodes** (the original form of this API):
  `{"http://example.org/Person": ["http://example.org/alice"]}`.

Without a shape map (absent, `""`, `[]` or `{}`), every shape declared in the
schema is checked against the nodes that have an arc one of its triple
constraints names. This discovery mode is a convenience of this API, not part
of ShEx.

Triple-pattern selectors and discovery read the same graphs as the
validation, so neither finds a node in a graph the caller may not read.

---

## What is implemented

The engine implements ShEx 2.1 (the Final Community Group Report of
2019-10-08):

- **ShExC** — the full grammar: `BASE`, `PREFIX`, `IMPORT`, `start=`, start
  actions, `EXTERNAL`, `AND` / `OR` / `NOT`, node constraints combined with
  shapes (`IRI @<S>`), value sets with IRI, literal and language stems,
  ranges and exclusions (`[<http://ex/>~ - <http://ex/private>~]`, `[@en~]`,
  `[. - "x"]`), string facets (`LENGTH`, `MINLENGTH`, `MAXLENGTH`,
  `/regex/flags`), numeric facets (`MININCLUSIVE` … `FRACTIONDIGITS`),
  `CLOSED`, `EXTRA`, `EachOf` (`;`), `OneOf` (`|`), bracketed groups with
  cardinalities (`*`, `+`, `?`, `{m}`, `{m,}`, `{m,n}`), inverse constraints
  (`^ex:p`), triple-expression labels and inclusions (`$ex:t`, `&ex:t`),
  annotations (`// ex:p "x"`) and semantic actions (`%ex:act{ … %}`).
  Keywords are case-insensitive. The ShEx 2.0 form `PATTERN "regex"` is also
  accepted.
- **ShExJ** — both the 2.1 layout (a shape expression carrying its `id`) and
  the `ShapeDecl` wrapper later drafts and the test suite use.
- **ShExR** — schemas stored as RDF in the `http://www.w3.org/ns/shex#`
  vocabulary, read for `IMPORT` (below).
- **Semantics** — the neighbourhood of a node is partitioned between the
  shape's triple constraints and the remainder (§5.5.2): every arc whose
  predicate (and direction) a triple constraint names must be matched by a
  constraint its value satisfies; an arc no constraint accepts may stay
  unmatched only when its predicate is `EXTRA`; a `CLOSED` shape allows no
  other arcs out. Groups, choices and cardinalities are decided on the
  assignment of arcs to constraints. Recursive shapes are evaluated as a
  greatest fixpoint per strongly connected component of the schema, and
  negation (`NOT`, and the negative dependency an `EXTRA` predicate creates)
  is stratified: a schema whose references cycle through a negation is
  rejected (§5.7.4).
- **Datatypes** — values are typed terms, not strings. `datatype` checks the
  lexical form of the XSD numeric types (with their derived integer ranges:
  `"300"^^xsd:byte` is not an `xsd:byte`), `xsd:boolean`, `xsd:dateTime`,
  `xsd:date` and `xsd:time`. Numeric facets compare `xsd:decimal` and the
  integer types exactly and use XPath type promotion for `xsd:float` /
  `xsd:double`. String facets count Unicode code points. Patterns are XPath
  regular expressions (flags `i`, `m`, `s`, `x`, `q`).

### Imports come from the store

`IMPORT <iri>` is resolved only from the store, never over the network: `<iri>`
must name a named graph the caller may read, holding the imported schema as
ShExR. Imports are transitive and circular imports are fine; an imported
schema's `start` is ignored, and an import that redeclares a label or has
start actions is an error (§5.6).

To make a schema importable, store its ShExR form (for example from a ShEx
tool's RDF output) in a named graph whose IRI you then import.

### Semantic actions

Semantic actions are parsed and kept, but code is **never executed**. Only the
test extension `http://shex.io/extensions/Test/` is evaluated, because it has
no code to run: `print(…)` succeeds and `fail(…)` fails the shape, group or
triple constraint it is attached to. Actions of any other extension succeed
without being looked at.

### Not implemented

- **ShEx 2.next** — `EXTENDS`, `ABSTRACT` and `RESTRICTS` are refused with a
  message naming them (in ShExC, ShExJ and ShExR).
- **`EXTERNAL` shapes** — ShEx leaves their definition to the implementation;
  this one has none, so a node never satisfies an `EXTERNAL` shape.
- **SPARQL node selectors** in shape maps.
- **Result shape maps** — the report is the JSON above, not a ShapeMap with
  `@!` entries.

### Things to know about stored data

- The store keeps typed literals exactly as written (since 2026-10-03):
  `"01"^^xsd:integer` stays `"01"`, and `xsd:byte` stays `xsd:byte`. A
  `datatype xsd:byte` constraint, a `LENGTH` facet on a number, or a value
  set listing `"1.0"^^xsd:decimal` see stored data as it was written. Data
  loaded by an earlier version keeps the canonical form it was stored in.
- Blank-node labels are the store's, not the uploaded document's, so string
  facets on blank nodes (allowed by ShEx, §5.4.4 note) test the store's
  labels.

---

## Conformance

The engine is run against the [shexTest](https://github.com/shexSpec/shexTest)
suite, vendored under `tests/fixtures/shextest` (W3C Software and Document
License; see `PROVENANCE.md` there), by `tests/shextest_conformance.rs`:

- the validation tests (ValidationTest must conform, ValidationFailure must
  not), with imports, external semantic-action code and shape maps;
- the representation tests (ShExC, ShExJ and ShExR forms of each schema parse
  to the same schema);
- the negative syntax and negative structure tests.

Tests tagged with a ShEx 2.next trait (`Extends`, `MultiExtends`,
`ExtendsDiamond`, `Abstract`), and six untagged representation tests of 2.next
schemas, are skipped. The runner is a two-way ratchet: every test not listed
as a known failure must pass, and every listed one must still fail.

At the pinned commit the engine passes all of the 2.1 part: 1209 validation,
462 representation, 105 negative-syntax and 19 negative-structure tests, with
no known failures (122 skipped as 2.next). Run through the store instead of
on the parsed data, every validation case answers the same: the runner's
`STORE_DIVERGENCES` list is empty (it held 40 lexical-form cases until the
store kept literals as written), and it fails if a case starts to diverge.

These are the project's own development and regression results, not a
conformance certification.
