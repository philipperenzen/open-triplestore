# Datatypes

Every RDF literal in the store carries a **datatype** — it tells you (and the
query engine) whether `"42"` is a piece of text, a whole number, or a year. This
page lists the datatypes the triplestore supports, explains how literals are
modelled, and lets you open runnable examples for each.

> Each section below has a collapsible **▸ View example** block with the Turtle
> you write, a SPARQL query you can run, and a **Try this query** link that opens
> it pre-filled in the [SPARQL editor](/sparql).

## How a literal is modelled

An RDF literal has up to three parts:

```
"410.0"^^<http://www.w3.org/2001/XMLSchema#decimal>
 └─ lexical form          └─ datatype IRI
```

- **Lexical form** — the text between the quotes, exactly as you write it.
- **Datatype IRI** — what the text *means*. Almost always an `xsd:` IRI.
- **Language tag** — only for text: `"Het jungleboek"@nl`. A language-tagged string
  always has the datatype `rdf:langString` (you never write that IRI yourself).

Two defaults save you typing:

| You write | Datatype it gets |
|---|---|
| `"plain text"` (no tag, no `^^`) | `xsd:string` |
| `"tekst"@nl` (a language tag) | `rdf:langString` |

Turtle also has literal **shorthands** so common values need no `^^`:

| Shorthand | Expands to |
|---|---|
| `true` / `false` | `xsd:boolean` |
| `42` | `xsd:integer` |
| `4.2` | `xsd:decimal` |
| `4.2e1` | `xsd:double` |

> **Value space vs. stored form.** The store keeps every literal **exactly as
> written**: its lexical form and its datatype. `"01"^^xsd:integer`,
> `"1"^^xsd:boolean`, `"5"^^xsd:nonNegativeInteger` and a `+00:00` time zone
> come back the way you wrote them, from SPARQL, the Graph Store Protocol and
> every download. Two things follow:
>
> - **Expressions compare values.** The engine (Oxigraph) implements the XSD
>   *value spaces*, so in a `FILTER`, `ORDER BY`, arithmetic or an aggregate,
>   numbers, booleans, dates and durations are compared and computed **by
>   value**: `"01"^^xsd:integer = 1`, `"5"^^xsd:int = 5` and
>   `"1.0E2"^^xsd:double = 100.0` are all true. A computed value is printed in
>   its canonical form (`?x + 0` gives `1`, not `01`).
> - **Graph patterns compare terms.** A triple pattern, a join between two
>   patterns, `DISTINCT`, `GROUP BY`, `sameTerm` and `DELETE DATA` match the
>   literal as written, as RDF defines it: `{ ?s ex:n 1 }` does not find
>   `"01"^^xsd:integer` or `"1"^^xsd:int`, `true` does not find
>   `"1"^^xsd:boolean`, and `SELECT DISTINCT` returns `"5"^^xsd:int` and `5` as
>   two values. To match by value, bind a variable and filter:
>   `{ ?s ex:n ?n FILTER(?n = 1) }`.
>
> Datatypes outside the XSD value space (see
> [Other & custom datatypes](#other-custom-datatypes)) compare by exact match
> in expressions too.
>
> Until the store kept lexical forms (see the changelog), it wrote the typed
> literals of those value spaces in a canonical form: `"01"` as `"1"`,
> `"1"^^xsd:boolean` as `true`, every type derived from `xsd:integer` as
> `xsd:integer`, `xsd:dateTimeStamp` as `xsd:dateTime`. Data loaded then still
> reads that way. Appending the same file again adds the literals as written
> beside the old ones, so replace a graph (`PUT`) rather than appending to it
> when you reload such data.

<details>
<summary>▸ View example — what datatypes are in my data?</summary>

This query inventories every literal datatype actually present, most common
first — a quick way to audit how a dataset is typed.

```sparql
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
SELECT ?datatype (COUNT(*) AS ?count) WHERE {
  ?s ?p ?o .
  FILTER(isLiteral(?o))
  BIND(DATATYPE(?o) AS ?datatype)
} GROUP BY ?datatype ORDER BY DESC(?count)
```

[Try this query](/sparql?query=PREFIX%20rdf%3A%20%3Chttp%3A%2F%2Fwww.w3.org%2F1999%2F02%2F22-rdf-syntax-ns%23%3E%0ASELECT%20%3Fdatatype%20(COUNT(*)%20AS%20%3Fcount)%20WHERE%20%7B%0A%20%20%3Fs%20%3Fp%20%3Fo%20.%0A%20%20FILTER(isLiteral(%3Fo))%0A%20%20BIND(DATATYPE(%3Fo)%20AS%20%3Fdatatype)%0A%7D%20GROUP%20BY%20%3Fdatatype%20ORDER%20BY%20DESC(%3Fcount))

</details>

## Text

| Datatype | Use for | Example literal |
|---|---|---|
| `xsd:string` | plain machine text, codes, identifiers | `"978-0-14-036122-9"` |
| `rdf:langString` | human-readable text in a language | `"Het jungleboek"@nl` |
| `xsd:anyURI` | a URI *as a value*, not as a node | `"mailto:info@example.org"^^xsd:anyURI` |

Other XSD text types (`xsd:token`, `xsd:normalizedString`, `xsd:Name`,
`xsd:language`, …) are accepted and stored, but are treated as text — they are
not given a separate value space.

> **Rule of thumb:** human-readable labels MUST carry a language tag (making them
> `rdf:langString`); machine codes and identifiers stay `xsd:string`. See the
> [Modelling Styleguide](/docs/linked-data-modelling-styleguide) §6.2.

<details>
<summary>▸ View example — language-tagged labels</summary>

```turtle
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .

ex:JungleBook
    skos:prefLabel "The Jungle Book"@en , "Het jungleboek"@nl ;
    ex:isbn        "978-0-14-036122-9" .   # xsd:string — a code, not a label
```

List every language-tagged value and its tag:

```sparql
SELECT ?s ?label (LANG(?label) AS ?lang) WHERE {
  ?s ?p ?label .
  FILTER(LANG(?label) != "")
} LIMIT 50
```

[Try this query](/sparql?query=SELECT%20%3Fs%20%3Flabel%20(LANG(%3Flabel)%20AS%20%3Flang)%20WHERE%20%7B%0A%20%20%3Fs%20%3Fp%20%3Flabel%20.%0A%20%20FILTER(LANG(%3Flabel)%20!%3D%20%22%22)%0A%7D%20LIMIT%2050)

</details>

## Numbers

| Datatype | Use for | Example literal |
|---|---|---|
| `xsd:integer` | whole numbers | `42` or `"42"^^xsd:integer` |
| `xsd:decimal` | exact decimal values (money, lengths) | `4.2` or `"410.0"^^xsd:decimal` |
| `xsd:double` | 64-bit floating point | `"4.2e0"^^xsd:double` |
| `xsd:float` | 32-bit floating point | `"4.2"^^xsd:float` |

The derived integer types are also value-typed: `xsd:long`, `xsd:int`,
`xsd:short`, `xsd:byte`, `xsd:nonNegativeInteger`, `xsd:positiveInteger`,
`xsd:negativeInteger`, `xsd:nonPositiveInteger`, and the `unsigned*` family. Use
them when a value is bounded or sign-constrained (e.g. a count is a
`xsd:nonNegativeInteger`). They compute as integers in expressions, but the
store keeps the datatype you wrote, so `DATATYPE(?n)` in a query still reports
`xsd:integer` (the value's type) while SHACL `sh:datatype xsd:nonNegativeInteger`
and a download see `xsd:nonNegativeInteger`.

> **Derived integer types are stored as `xsd:integer`.** The engine keeps the
> value, not the declared type: `"5"^^xsd:nonNegativeInteger` reads back as
> `"5"^^xsd:integer`, and an out-of-range `"300"^^xsd:byte` is accepted and reads
> back as `"300"^^xsd:integer` (`xsd:dateTimeStamp` likewise becomes
> `xsd:dateTime`). A SHACL `sh:datatype xsd:nonNegativeInteger` constraint
> therefore fails on such stored data; constrain it with `sh:datatype
> xsd:integer` plus `sh:minInclusive 0` instead.

> Prefer `xsd:decimal` over `xsd:double` for quantities you compare for equality
> (`xsd:double` is subject to floating-point rounding).

<details>
<summary>▸ View example — typed numbers and a range filter</summary>

```turtle
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:JungleBook
    ex:weightGrams      "410.0"^^xsd:decimal ;
    ex:publicationYear  1894 ;                       # xsd:integer (shorthand)
    ex:rating            4 .
```

Find subjects whose `xsd:decimal` value exceeds 100 (compared **by value**):

```sparql
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT ?s ?v WHERE {
  ?s ?p ?v .
  FILTER(DATATYPE(?v) = xsd:decimal && ?v > 100)
} LIMIT 50
```

[Try this query](/sparql?query=PREFIX%20xsd%3A%20%3Chttp%3A%2F%2Fwww.w3.org%2F2001%2FXMLSchema%23%3E%0ASELECT%20%3Fs%20%3Fv%20WHERE%20%7B%0A%20%20%3Fs%20%3Fp%20%3Fv%20.%0A%20%20FILTER(DATATYPE(%3Fv)%20%3D%20xsd%3Adecimal%20%26%26%20%3Fv%20%3E%20100)%0A%7D%20LIMIT%2050)

</details>

## Boolean

| Datatype | Use for | Example literal |
|---|---|---|
| `xsd:boolean` | yes/no flags | `true`, `false`, or `"true"^^xsd:boolean` |

In Turtle, the bare keywords `true` and `false` are `xsd:boolean` literals.

## Dates, times and partial dates

All members of the XSD date/time family are value-typed (correctly ordered and
comparable), and `ADJUST()` can shift them across timezones.

| Datatype | Use for | Example literal |
|---|---|---|
| `xsd:dateTime` | a timestamp | `"2025-01-15T10:00:00Z"^^xsd:dateTime` |
| `xsd:date` | a calendar date | `"1996-11-06"^^xsd:date` |
| `xsd:time` | a time of day | `"10:00:00"^^xsd:time` |
| `xsd:gYear` | a year | `"1996"^^xsd:gYear` |
| `xsd:gYearMonth` | a year-month | `"1996-11"^^xsd:gYearMonth` |
| `xsd:gMonth` / `xsd:gMonthDay` / `xsd:gDay` | recurring partial dates | `"--11-06"^^xsd:gMonthDay` |

Durations are value-typed too: `xsd:duration`, `xsd:dayTimeDuration`,
`xsd:yearMonthDuration` (e.g. `"P1DT2H30M"^^xsd:dayTimeDuration`).

<details>
<summary>▸ View example — dates and a date-range filter</summary>

```turtle
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix dct: <http://purl.org/dc/terms/> .

ex:Survey
    dct:issued "2025-01-15T10:00:00Z"^^xsd:dateTime ;
    ex:openedOn "1996-11-06"^^xsd:date .
```

Find dates on or after 2020, sorted chronologically:

```sparql
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT ?s ?d WHERE {
  ?s ?p ?d .
  FILTER(DATATYPE(?d) = xsd:date && ?d >= "2020-01-01"^^xsd:date)
} ORDER BY ?d LIMIT 50
```

[Try this query](/sparql?query=PREFIX%20xsd%3A%20%3Chttp%3A%2F%2Fwww.w3.org%2F2001%2FXMLSchema%23%3E%0ASELECT%20%3Fs%20%3Fd%20WHERE%20%7B%0A%20%20%3Fs%20%3Fp%20%3Fd%20.%0A%20%20FILTER(DATATYPE(%3Fd)%20%3D%20xsd%3Adate%20%26%26%20%3Fd%20%3E%3D%20%222020-01-01%22%5E%5Exsd%3Adate)%0A%7D%20ORDER%20BY%20%3Fd%20LIMIT%2050)

</details>

## Binary

| Datatype | Use for | Example literal |
|---|---|---|
| `xsd:base64Binary` | inline binary, base64-encoded | `"SGVsbG8="^^xsd:base64Binary` |
| `xsd:hexBinary` | inline binary, hex-encoded | `"48656C6C6F"^^xsd:hexBinary` |

For real files, prefer the asset/upload pipeline over inline binary literals.

## Spatial (GeoSPARQL)

Geometry is stored as a **WKT literal** with the datatype `geo:wktLiteral`
(`http://www.opengis.net/ont/geosparql#wktLiteral`). These literals feed the
GeoSPARQL functions (`geof:distance`, `geof:sfWithin`, …).

| Datatype | Use for | Example literal |
|---|---|---|
| `geo:wktLiteral` | a geometry in Well-Known Text | `"POINT(4.4870 51.9094)"^^geo:wktLiteral` |

WKT coordinate order is **`POINT(lon lat)`**. A literal may be prefixed with a
CRS IRI: `"<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(4.49 51.91)"`.
See [GeoSPARQL](/docs/geosparql) for the spatial functions.

<details>
<summary>▸ View example — a point geometry</summary>

The canonical blank-node geometry shape (here, a book's place of publication):

```turtle
@prefix geo: <http://www.opengis.net/ont/geosparql#> .

ex:JungleBook geo:hasGeometry [
    geo:asWKT "POINT(4.4870 51.9094)"^^geo:wktLiteral
] .
```

List every geometry literal in the store:

```sparql
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
SELECT ?feature ?wkt WHERE {
  ?feature geo:hasGeometry/geo:asWKT ?wkt .
} LIMIT 50
```

[Try this query](/sparql?query=PREFIX%20geo%3A%20%3Chttp%3A%2F%2Fwww.opengis.net%2Font%2Fgeosparql%23%3E%0ASELECT%20%3Ffeature%20%3Fwkt%20WHERE%20%7B%0A%20%20%3Ffeature%20geo%3AhasGeometry%2Fgeo%3AasWKT%20%3Fwkt%20.%0A%7D%20LIMIT%2050)

</details>

## Other & custom datatypes

Any IRI may be a datatype. The store accepts and round-trips it unchanged;
because it is outside the XSD value space, two such literals are equal only when
**both** their lexical form and datatype IRI match exactly.

| Datatype | Use for |
|---|---|
| `rdf:HTML` | a fragment of HTML markup |
| `rdf:XMLLiteral` | a fragment of XML |
| `rdf:JSON` | an embedded JSON value |
| *your own IRI* | a domain-specific lexical format you define and validate |

<details>
<summary>▸ View example — a custom datatype</summary>

```turtle
@prefix ex: <https://example.org/dt/> .

ex:coverImage ex:colourCode "#1f6feb"^^ex:hexColour .
```

`DATATYPE(?o)` returns your IRI verbatim; constrain its lexical form with a
SHACL `sh:pattern` (see [SHACL Validation](/docs/shacl)).

</details>

## How datatypes appear in query results

In SPARQL Results JSON, a literal is an object with a `value` plus, when
relevant, a `datatype` or language. `xsd:string` is the default and is **omitted**:

```json
{ "type": "literal", "value": "978-0-14-036122-9" }
{ "type": "literal", "value": "Het jungleboek", "xml:lang": "nl", "language": "nl" }
{ "type": "literal", "value": "410.0",
  "datatype": "http://www.w3.org/2001/XMLSchema#decimal" }
```

- `value` is always the lexical form.
- A language-tagged string carries both `xml:lang` and `language` (the datatype
  is implicitly `rdf:langString`).
- Any datatype other than `xsd:string` is reported in `datatype`.

## Working with datatypes

**Querying.** `DATATYPE(?lit)` returns the datatype IRI; `LANG(?lit)` returns the
language tag; `STRDT("4.2", xsd:decimal)` and `STRLANG("tekst", "nl")` build typed
or tagged literals; and casts like `xsd:integer(?x)` or `xsd:dateTime(?x)` convert
between value spaces.

**Validating.** SHACL constrains the datatype of a value with `sh:datatype`, and
language coverage with `sh:languageIn` / `sh:uniqueLang`:

```turtle
ex:LabelShape sh:property [
    sh:path skos:prefLabel ;
    sh:datatype rdf:langString ;
    sh:uniqueLang true ;
] .
```

See [SHACL Validation](/docs/shacl).

**Importing & mapping.** When data arrives by upload or SPARQL, datatypes are
read straight from the source syntax (see [RDF Formats](/docs/formats) and
[Import Auto-Detection](/docs/import)). RML mappings set a literal's type with
`rr:datatype` / `rr:language`.

**Parameterised APIs.** A saved query exposed as an API declares each variable's
type so supplied values are validated and rendered into SPARQL safely. The
parameter types are: **IRI**, **string**, **integer**, **decimal**, **boolean**,
**date** (`YYYY-MM-DD`) and **dateTime** (ISO-8601). See
[API Services & AI Queries](/docs/api-services).

## Notes & limits

- **Triple terms** (`<<( ex:a ex:b ex:c )>>`, RDF 1.2) are a *term kind*, not a
  literal datatype, but they are supported — see the RDF 1.2 row in
  [Supported Standards](/docs/standards). In RDF 1.2, `<< ex:a ex:b ex:c >>` is
  shorthand for a reifier that points at such a triple term, not a term itself.
- **Base-direction strings** (`"text"@ar--rtl`, datatype `rdf:dirLangString`)
  are stored and queried as RDF 1.2 defines them, with `LANGDIR`, `hasLANGDIR`
  and `STRLANGDIR`. One known gap: the in-memory columnar copy that answers
  some queries drops the direction.
- No literal is rewritten or canonicalised in storage: every one is returned
  byte-for-byte as you stored it. Only values an expression computes are
  printed in canonical form.

Related: [Linked Data Modelling](/docs/modelling) ·
[Modelling Styleguide](/docs/linked-data-modelling-styleguide) ·
[RDF Formats](/docs/formats) · [GeoSPARQL](/docs/geosparql) ·
[SHACL Validation](/docs/shacl) · [Supported Standards](/docs/standards)
