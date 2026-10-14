# Draft upstream PR: custom aggregates with several arguments (spareval)

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead. Third of three; it depends on the spargebra and
sparopt ones.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

spareval: evaluate every argument of a custom aggregate call

## Body

A custom aggregate called with several arguments (`ex:agg(?x, 0.5)`) runs over
the list of their values per solution (SPARQL 1.1 §18.5.1). The accumulator
API only takes one value.

### Change

- `AggregateFunctionAccumulator` gets a provided method
  `accumulate_arguments(&mut self, arguments: Vec<Term>) -> bool`, called
  instead of `accumulate` for a call with more than one argument. Its default
  takes exactly one argument; `false` makes the aggregate's result unbound (as
  an argument that does not evaluate does). Existing accumulators compile
  unchanged and refuse extra arguments.
- The evaluator builds one expression evaluator per argument and a new
  accumulator wrapper that evaluates all of them per solution, drops the
  aggregate on an error in any of them, and under `DISTINCT` de-duplicates whole
  argument lists.
- A built-in aggregate with extra arguments (only reachable by building the
  algebra by hand) is an evaluation error.

### Tests

A two-argument custom aggregate with a constant and with a per-group
argument; disagreeing values make the result unbound; `DISTINCT` over argument
lists; one-argument calls unchanged.
