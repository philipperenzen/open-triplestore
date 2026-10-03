# W3C RDF 1.1 Semantics test cases (`rdf-mt`): vendored copy

- Source: https://github.com/w3c/rdf-tests (`rdf/rdf11/rdf-mt`)
- Commit: 369a90d1a60c021b746df2e411da0ff36258a758, the commit the SPARQL 1.1
  sections in `../w3c-sparql11/` come from (vendored 2026-10-03).
- Changes: none. Every file here, `LICENSE` and `README` included, is
  byte-identical to `rdf/rdf11/rdf-mt/` at that commit (checked file by file
  against `raw.githubusercontent.com` on 2026-10-03). This file and
  `LICENSE.md` are ours.
- Licence: as the SPARQL subset. Upstream offers the tests under the W3C Test
  Suite License or the W3C 3-clause BSD License; this copy relies on the BSD
  option (see `LICENSE.md`). Not covered by this project's licence.
- Scope: the whole `rdf-mt` directory: the manifest and every test document.
- Runner: `tests/w3c_rdf_mt_manifests.rs`. It loads each action into a store,
  materializes it with the RDFS engine for the `RDF` and `RDFS` regimes (none
  for `simple`), and checks whether the result graph, blank nodes read as
  variables, matches. Known gaps are its KNOWN_FAILURES list (a two-way
  ratchet), summarised in `docs/conformance/entailment.md`. No score is
  published: W3C's test-suite policy allows no performance claims for a
  subset or partial run.

To check a file:

```bash
curl -sSf https://raw.githubusercontent.com/w3c/rdf-tests/369a90d1a60c021b746df2e411da0ff36258a758/rdf/rdf11/rdf-mt/manifest.ttl | shasum -a 256
shasum -a 256 manifest.ttl
```
