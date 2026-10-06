# SWRL Rules

SWRL (the [Semantic Web Rule Language](https://www.w3.org/submissions/SWRL/)) adds
Horn-clause rules to OWL: *if the body holds, the head holds*. The server reads SWRL in
six syntaxes, evaluates every built-in of the submission's §8 natively, and runs rules
either on request (`POST /api/swrl/execute`) or stored with a dataset, where they run
with the dataset's entailment regime after every write.

```text
ex:Person(?p) ^ ex:age(?p, ?a) ^ swrlb:add(?next, ?a, 1) -> ex:nextAge(?p, ?next)
```

The feature is compiled in with the `swrl` cargo feature (part of `full`).

## Syntaxes

Pick one with `format`:

| `format` | Syntax | Notes |
|---|---|---|
| `text` (default) | `http://ex/A(?x) ^ http://ex/p(?x, ?y) -> http://ex/B(?y)` | Absolute IRIs only; no built-ins, data ranges or class expressions. |
| `xml` (or `owlxml`) | OWL/XML `DLSafeRule`, as the OWL API and Protégé write it | `Prefix` declarations, `abbreviatedIRI`, `xml:base`, every `Literal` form, `BuiltInAtom` and `BuiltinAtom`. |
| `rdf` | The SWRL RDF syntax (§5): `swrl:Imp` with `swrl:body` / `swrl:head` lists | Any RDF serialisation: `rdf_format` is `turtle` (default), `ntriples`, `nquads`, `trig`, `rdfxml`, `jsonld`, `n3` or a media type. `base_iri` resolves relative IRIs. |
| `functional` | OWL 2 functional-syntax `DLSafeRule`, inside an `Ontology(…)` or on its own | `Prefix(…)` declarations; other axioms are skipped. |
| `swrlapi` | The SWRLAPI human-readable syntax (Protégé's SWRL tab) | Prefixes from the request's `prefixes` (`""` is the default prefix), then the server's prefix registry. `xsd:integer(?v)` is a data range atom. |
| `ruleml` | The SWRL §4 XML concrete syntax (`ruleml:imp`, `swrlx:*Atom`) | `owlx:` class descriptions and `datarangeAtom`. DTD entities such as `&swrlb;` are not expanded: write full IRIs. |

`rdf:`, `rdfs:`, `xsd:`, `owl:`, `swrl:` and `swrlb:` are predeclared in every syntax that
has prefixes.

## Atoms

| Atom | Holds when |
|---|---|
| `ClassAtom(C, x)` | `x rdf:type C`, for a named class `C` — or `x` is a member of the class expression `C` (below) |
| `ObjectPropertyAtom(p, x, y)` | `x p y`, `y` an individual; `ObjectInverseOf(p)` swaps the arguments |
| `DataPropertyAtom(p, x, v)` | `x p v`, `v` a literal |
| `SameIndividualAtom(x, y)` / `DifferentIndividualsAtom(x, y)` | `x owl:sameAs y` / `x owl:differentFrom y` |
| `DataRangeAtom(D, v)` | `v` is in the data range `D` (below) |
| `BuiltInAtom(b, args…)` | the built-in `b` holds of its arguments (below) |

Argument positions are typed: a literal where an individual belongs, an individual where a
data value belongs, or one variable used as both is refused. The text and SWRLAPI syntaxes
cannot say whether a property is an object or a data property, so a property atom whose
second argument is a variable matches individuals and literals alike there.

### Data ranges

A `DataRangeAtom` is evaluated by the server itself:

- **A datatype** holds the literals in its value space, by value: an `xsd:byte` 5 is in
  `xsd:integer`, `xsd:decimal`, `owl:rational` and `owl:real`, and `"5.0"^^xsd:decimal` is in
  `xsd:integer`. `xsd:float` and `xsd:double` are disjoint from the decimals and from each
  other, as in the OWL 2 datatype map. An ill-typed literal (`"x"^^xsd:integer`) is in no
  datatype but `rdfs:Literal`. Known datatypes: the XSD numeric, string, boolean, date/time,
  duration, `g*`, binary and `anyURI` types, `rdf:PlainLiteral`, `rdf:langString`,
  `rdf:XMLLiteral`, `rdfs:Literal`, `owl:real`, `owl:rational`. A range over any other
  datatype is refused.
- **`DatatypeRestriction`** facets: `minInclusive`, `maxInclusive`, `minExclusive`,
  `maxExclusive` (numbers, dates, times, durations), `length`, `minLength`, `maxLength`
  (characters, octets for binary types), `pattern` (anchored), `totalDigits`,
  `fractionDigits` and `rdf:langRange`.
- **`DataOneOf`** holds the listed values, compared by value; **`DataUnionOf`**,
  **`DataIntersectionOf`** and **`DataComplementOf`** combine ranges (a complement holds
  every literal outside its range). An individual is in no data range.

The variable of a data range atom must be bound by another atom, except over a finite
range (`DataOneOf`, or a union of them), which binds it to each listed value in turn.

### Class expressions

A `ClassAtom` may hold any OWL 2 class expression: `ObjectIntersectionOf`,
`ObjectUnionOf`, `ObjectComplementOf`, `ObjectOneOf`, `ObjectSomeValuesFrom`,
`ObjectAllValuesFrom`, `ObjectHasValue`, `ObjectHasSelf`, `ObjectMin/Max/ExactCardinality`
and the `Data…` restrictions. The expression is not evaluated as a pattern. It becomes an
*auxiliary class* `urn:ots:swrl:aux:<hash>` (the hash of its functional-syntax form), the
axiom `aux owl:equivalentClass <expression>` (in the OWL 2 RDF mapping) is written to the
run's target graph, and the atom matches members of `aux`. The entailment regime the
rules run with materialises who those members are; in the head, asserting `x rdf:type aux`
lets the regime derive what the expression implies (an intersection's members, an
`ObjectHasValue`'s value).

So class-expression atoms need a regime: pass `"regime"` to `/api/swrl/execute`, or store
the rules with a dataset whose regime is in `materialize` mode. OWL 2 RL covers the
RL fragment of class expressions (intersections, unions and existentials in the body,
intersections, universals and `ObjectHasValue` in the head, …); what RL cannot infer
(a complement's members, an existential in the head) needs a DL regime with a DL
backend. A rule with a class-expression atom and no regime is refused with a message
naming the expression.

## Built-ins

All the `swrlb:` built-ins of the SWRL submission §8 are evaluated natively, in Rust, over
the solutions of the rule's other body atoms, using the XPath/XSD value types Oxigraph
itself uses (`oxsdatatypes`) and the `regex` crate. A built-in outside `swrlb:` is
refused by name, as is one with the wrong number of arguments.

Which arguments are bound when a built-in is reached decides what it does:

| Mode | When | Example |
|---|---|---|
| **check** | every argument bound | `swrlb:greaterThan(?age, 17)` |
| **bind** | the first argument unbound, the rest bound: it is computed | `swrlb:add(?next, ?age, 1)` binds `?next` |
| **split** | a constructor's first argument bound, components unbound | `swrlb:dateTime(?t, ?y, ?mo, ?d, ?h, ?mi, ?s, ?tz)` binds the components |
| **enumerate** | one solution per value | `swrlb:tokenize(?tok, ?s, ",")`, `swrlb:member(?e, ?list)`, `swrlb:sublist(?l, ?sub)` |
| **solve** | one operand unbound, the rest bound | `swrlb:add(10, ?x, 3)` binds `?x` to 7; also `subtract`, `unaryPlus`, `unaryMinus`, `booleanNot`, `equal` |

The engine orders a rule's built-ins so each runs once what it needs is bound. A pattern
with infinitely many solutions (`swrlb:add(?z, ?x, ?y)` with two unbound operands,
`swrlb:lessThan(?a, ?b)` with an unbound side, `swrlb:member` over an unbound list) is
refused before anything runs, naming the built-in and the unbound arguments. So is a
function with its first argument bound and another one unbound when it is not one of
the built-ins that solve (`swrlb:multiply(6, ?x, 2)`): bind the operand in another atom.

A built-in applied to values of the wrong type (`swrlb:add` over a string, a date
function over a number) does not hold: no error, no derivation. Numbers follow XPath
promotion (integer → decimal → float → double); integer by integer division is a
decimal; every operation is checked, so an overflow or a division by zero makes the
built-in false rather than wrapping.

| § | Built-ins | Modes |
|---|---|---|
| 8.1 comparisons | `equal`, `notEqual`, `lessThan`, `lessThanOrEqual`, `greaterThan`, `greaterThanOrEqual` | check; `equal` also binds either side. Literals compare by value (numbers across types, strings, booleans, dates, times, durations); `equal`/`notEqual` on IRIs compare the terms |
| 8.2 math | `add` and `multiply` (any number of operands), `subtract`, `divide`, `integerDivide`, `mod`, `pow`, `unaryPlus`, `unaryMinus`, `abs`, `ceiling`, `floor`, `round`, `roundHalfToEven` (optional precision), `sin`, `cos`, `tan` | check, bind; `add`, `subtract`, `unaryPlus`, `unaryMinus` also solve |
| 8.3 boolean | `booleanNot` | check, bind, solve |
| 8.4 strings | `stringConcat`, `substring`, `stringLength`, `normalizeSpace`, `upperCase`, `lowerCase`, `translate`, `substringBefore`, `substringAfter`, `replace`, `tokenize` | check, bind (`tokenize` one token per solution) |
| | `stringEqualIgnoreCase`, `contains`, `containsIgnoreCase`, `startsWith`, `endsWith`, `matches` | check |
| 8.5 dates, times, durations | `yearMonthDuration`, `dayTimeDuration`, `dateTime`, `date`, `time` | check, bind, split; the timezone argument is optional, a string (`"Z"`, `"+01:00"`, `""` for none) or a `dayTimeDuration` |
| | `addYearMonthDurations`, `subtractYearMonthDurations`, `multiplyYearMonthDuration`, `divideYearMonthDurations`, `addDayTimeDurations`, `subtractDayTimeDurations`, `multiplyDayTimeDurations`, `divideDayTimeDuration`, `subtractDates`, `subtractTimes`, `add`/`subtract` `YearMonthDuration`/`DayTimeDuration` `To`/`From` `DateTime`/`Date` (8), `addDayTimeDurationToTime`, `subtractDayTimeDurationFromTime`, `subtractDateTimesYieldingYearMonthDuration`, `subtractDateTimesYieldingDayTimeDuration` | check, bind |
| 8.6 URIs | `resolveURI` | check, bind |
| | `anyURI` (scheme, host, port, path, query, fragment) | check, bind, split; components are strings, `""` when absent |
| 8.7 lists | `listConcat`, `listIntersection`, `listSubtraction`, `length`, `first`, `rest` | check, bind |
| | `member`, `sublist` | check, enumerate |
| | `empty` | check; binds `rdf:nil` |

Spelling variants with and without a trailing `s` (`multiplyYearMonthDurations`,
`divideYearMonthDuration`, …) are accepted. Durations are written as
`xsd:yearMonthDuration` and `xsd:dayTimeDuration`; inputs may be `xsd:duration` too.

**Regular expressions** (`matches`, `replace`, `tokenize`, the `pattern` facet) take XPath
flags `s`, `m`, `i`, `x` and `q`. XPath-only constructs — `\i`, `\c` and character-class
subtraction (`[a-z-[aeiou]]`) — are refused rather than read as something else, and a
pattern that does not compile refuses the rule. `replace` takes `$1`-style group
references and `\$` / `\\` escapes. A pattern that matches the empty string makes
`replace` and `tokenize` false, as in XPath.

**Lists** are RDF lists (`rdf:first` / `rdf:rest`) in the graphs the rule reads, compared
element by element. The constructive built-ins (`listConcat`, `listIntersection`,
`listSubtraction`, `rest` of a new list, `sublist` when it enumerates) mint list nodes
`urn:ots:swrl:list:<hash>`, the hash taken over the elements. The same list is always the
same node, so a rule that rebuilds it derives nothing new and the fixed point still
terminates. When a minted list lands in a head triple, its cells are written to the target
graph with it.

## Running rules: `POST /api/swrl/execute`

```json
{
  "rules": "ex:Person(?p) ^ ex:age(?p, ?a) ^ swrlb:greaterThanOrEqual(?a, 18) -> ex:Adult(?p)",
  "format": "swrlapi",
  "prefixes": { "ex": "http://example.org/" },
  "dataset": "people"
}
```

| Field | Meaning |
|---|---|
| `rules`, `format` | The rules and their syntax; `rdf_format`, `base_iri` and `prefixes` as in the table above. |
| `dataset`, `source_graphs` | What rule bodies read: the dataset's reasoning sources and explicit graphs, each read-checked as for `/api/reasoning/materialize`. Neither: the unnamed default graph. |
| `target_graph` | Where derived triples go: an absolute IRI the caller may write. Default: the dataset's inference graph with a `dataset`, else the default graph. |
| `regime` | `rdfs`, `owl2-rl`, `owl2-el`, `owl2-ql` or `owl2-dl`: run the rules and the regime to one joint fixed point in the target graph. Needs a scope (`dataset` or `source_graphs`) and a target. Class-expression atoms need it. |
| `max_iterations` | Fixed-point iterations of the rules (default 100, at most 1000). |

Every rule is compiled before any runs. One the server cannot run as written refuses the
whole request with `400` and nothing is written: an element it does not understand, an
unsafe rule (a head variable that occurs nowhere in the body), a built-in in the head, a
data range in the head, an unknown built-in or data range, an infinite built-in pattern, a
class expression without a regime, a literal where an individual belongs.

The response reports `rules_count`, `iterations`, `triples_inferred` (what the rules wrote
to the target graph), `converged` and `stop_reason` (`fixpoint`, `max_iterations` or
`timeout`), `target_graph`, `sources` (the graphs rule bodies read; `null` for the default
graph) and per-rule `rule_results`. A rule without built-ins runs as one SPARQL
`INSERT … WHERE …`; a rule with built-ins or data ranges shows the `SELECT` over its other
body atoms, followed by the built-ins it evaluates natively. With a `regime` the response
adds `regime`, `rounds` (rules-then-regime) and `regime_triples`. Execution counts against
the server's limit on concurrent expensive operations and stops at the write timeout.

## Rules stored with a dataset

`swrl:Imp` rules (the RDF syntax) in a dataset's `entailment`- and `model`-role graphs, and
in the model version it conforms to, always run, after every write to one of the dataset's
graphs. With a regime in `materialize` mode the rules and the regime run to one joint fixed
point in the regime's graph; without one, the rules run alone into
`urn:entailment:swrl:<dataset>`. `GET /api/datasets/{id}/entailment` reports the inference
graph and the last run of the rules (see [OWL Reasoning](/docs/reasoning#rules-stored-with-a-dataset)).

## Semantics

- **Sound.** A rule derives its head only for bindings that satisfy every body atom; an
  atom or built-in the server cannot evaluate refuses the rule instead of being dropped.
- **Complete for DL-safe Horn rules relative to the regime's materialisation.** Variables
  bind to the named individuals and literals in the graphs read (DL safety); the fixed
  point of rules and regime contains every head instance those facts support.
- **Incomplete under disjunctive DL reasoning.** Rules run against the regime's
  *materialised* consequences. A conclusion that needs reasoning by cases (an individual in
  `A ⊔ B` with a rule for each) is not derived, as the rules are not handed to the DL
  backend. HermiT supports DL-safe rules without built-ins; passing rules to it is future
  work.

See also: [OWL Reasoning](/docs/reasoning), [Supported Standards](/docs/standards).
