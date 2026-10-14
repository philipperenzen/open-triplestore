# Draft upstream PR: custom aggregates with several arguments (sparopt)

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead. Second of three; it depends on the spargebra one.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

sparopt: carry a custom aggregate's extra arguments through the algebra

## Body

With spargebra's `AggregateExpression::FunctionCall { extra_args, .. }`, the
optimizer algebra needs the same field, or the arguments after the first are
lost between parsing and evaluation.

### Change

`sparopt::algebra::AggregateExpression::FunctionCall` gets `extra_args:
Vec<Expression>`; both conversions (from and to spargebra's algebra) map it.
No optimizer pass rewrites aggregate arguments, so nothing else changes.
