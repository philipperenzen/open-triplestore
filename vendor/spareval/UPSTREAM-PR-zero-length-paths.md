# Draft upstream PR: zero-length paths with a constant endpoint

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead.

**Target:** `main`, with a backport to the `0.5` branch (the patch below is
written against `spareval` 0.2.7 and applies to `main`'s path evaluator with
context changes only).

## Title

spareval: a zero-length path matches a constant endpoint that is not in the graph

## Body

`?s :p* :o`, `:s :p* ?o`, `:s :p? ?o` and `?s :p? :o` return no solution when the
constant endpoint is not a subject or object of the active graph, and
`ASK { :x :p* :x }` is false on an empty graph. SPARQL 1.1 §18.6 evaluates the
zero-length path against a term endpoint without looking at the graph:

- `eval(Path(X:term, ZeroLengthPath, Y:term)) = { {} }` if X and Y are the same
  term, `{}` otherwise;
- `eval(Path(X:var, ZeroLengthPath, Y:term)) = { { (X, Y) } }` (and the mirror
  case);
- only `eval(Path(X:var, ZeroLengthPath, Y:var))` ranges over `nodes(G)`.

`ZeroOrMorePath` and `ZeroOrOnePath` include the zero-length path, so with a
constant start `:s` the answer always contains `?o = :s`.

The four W3C tests `property-path/manifest#zero_or_more_set_start`, `…_end`,
`zero_or_one_set_start` and `…_end` check exactly this on an empty dataset; they
are on `testsuite/tests/sparql.rs`'s ignore list ("Our property path handling is
wrong").

### Cause

`PathEvaluator::eval_from` / `eval_to` wrap `ZeroOrMore` and `ZeroOrOne` in
`run_if_term_is_a_graph_node`, and `eval_closed` answers `start == end` with
`is_subject_or_object`. That membership check is right when the endpoint value
comes from a variable (the spec's `nodes(G)` case: `VALUES ?x { :a } ?x :p* ?y`
on an empty graph has no solution), but the path evaluator does not know whether
its endpoint was a variable or a term of the query.

### Change

- In the `GraphPattern::Path` evaluator, note whether each endpoint selector is a
  constant (`TupleSelector::Constant`).
- Add `eval_from_term`, `eval_to_term` and `eval_closed_term`, used for a constant
  endpoint. They skip the membership check for the zero-length step at that
  endpoint and otherwise delegate to the existing functions. The "is a term"
  property follows the endpoint through `^` (swapped), `|`, `?` and the first step
  of `+`, and stops at the middle of `/`, which §18.4 translates into a fresh
  variable (`:x :p*/:q* ?y` on an empty graph stays empty).
- Variable endpoints keep the current behaviour, including when the variable is
  bound by the input.

### Tests

The four W3C entries above pass and can leave the ignore list. Extra cases worth
adding to `oxigraph-tests` (all on an empty or unrelated graph):

| Query | Expected |
| --- | --- |
| `ASK { <x> <p>* <x> }` | true |
| `ASK { <x> <p>? <x> }` | true |
| `ASK { <x> ^<p>* <x> }` | true |
| `ASK { <x> (<p>\|<q>)* <x> }` | true |
| `SELECT ?o { <x> <p>* ?o }` | `?o = <x>` |
| `SELECT ?s { ?s <p>? <x> }` | `?s = <x>` |
| `SELECT ?y { <x> <p>*/<q>* ?y }` | no solution (the middle is a variable) |
| `SELECT ?y { VALUES ?x { <x> } ?x <p>* ?y }` | no solution (variable endpoint) |
| `ASK { <x> <p>+ <x> }` | false |
