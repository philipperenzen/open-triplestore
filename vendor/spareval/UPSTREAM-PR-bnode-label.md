# Draft upstream issue and PR: BNODE(str) is fresh per solution

Not posted. A draft for the Oxigraph maintainers, to be filed only with the
project owner's go-ahead. Upstream does not track this yet (the W3C entry is not
on the testsuite ignore list), so it would start as an issue with this PR
attached.

**Target:** `main`, with a backport to the `0.5` branch.

## Title

spareval: BNODE(str) returns one node per solution, not one per string

## Body

`BNODE("x")` is implemented as `BlankNode::new("x")`, so:

1. the same string gives the same blank node in every solution of a query, and in
   every later query and update (`INSERT { ?s :p ?b } WHERE { … BIND(BNODE("x") AS ?b) }`
   run twice links everything to one node, and that node can also collide with a
   blank node already in the store whose label happens to be `x`);
2. a string that is not a legal blank-node label (`BNODE("100%")`, `BNODE("")`)
   gives an unbound result.

SPARQL 1.1 §17.4.2.9: "The BNODE function constructs a blank node that is
distinct from all blank nodes in the dataset being queried and distinct from all
blank nodes created by calls to this constructor for other query solutions. …
every call results in distinct blank nodes for different simple literals, and the
same blank node for calls with the same simple literal within expressions for one
solution mapping." Any simple literal is allowed.

The W3C test `functions/manifest#bnode01` checks the per-solution part; the
upstream testsuite passes it only because it compares results up to blank-node
renaming without checking co-reference across rows (worth confirming).

### Change

- `ExpressionEvaluatorContext` (crate-private) gains
  `build_blank_node_for_label`, returning the node for a label in a solution.
- `SimpleEvaluator` holds, per evaluation, two SipHash keys drawn at random
  (`RandomState`) and the set of variable positions that an `Extend` assigns. The
  node for `(solution, label)` is `BlankNode::new_from_unique_id` of a 128-bit
  keyed hash of the label and of the solution's bound positions, leaving out the
  `Extend`-assigned ones: those become bound while the solution is extended, so
  `BIND(BNODE("x") AS ?a) BIND(BNODE("x") AS ?b)` must agree although the second
  call sees `?a`. The result: the same node within a solution, a different node
  for another solution or another evaluation, any string accepted, and no
  realistic collision with the dataset's nodes (random keys per evaluation).
- The standalone expression evaluator (`QueryEvaluator::evaluate_expression`)
  hashes the label with its own random keys: one evaluation is one solution.

Known limit: two solutions that bind every non-`Extend` variable to the same
terms (true duplicates in the multiset) get the same node.

### Tests

`bnode01` passes with co-reference checked across rows. Extra cases:
`SELECT (BNODE("x") AS ?a) (BNODE("x") AS ?b) (BNODE("y") AS ?c) { VALUES ?i { 1 2 } }`
gives `?a = ?b ≠ ?c` within each row and different nodes across the two rows;
`BNODE("100%")` is bound; running the same `INSERT … BNODE("x")` twice creates two
nodes.
