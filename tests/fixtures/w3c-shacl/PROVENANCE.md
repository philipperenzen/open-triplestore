# W3C SHACL test suite — vendored copy

- Source: https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/core`)
- Commit: b6e73695d6196f33d7ce3ba47094a10fbc298e65 (vendored 2026-06-10)
- License: see LICENSE.md (W3C Software and Document License)
- Runner: `tests/w3c_shacl_conformance.rs` — each test file is self-contained
  (data + shapes + `mf:Manifest` entry + expected `sh:ValidationReport`).
  Comparison level: `sh:conforms` plus the multiset of violation focus nodes
  (see the runner header for rationale). Known gaps are tracked in the runner's
  KNOWN_FAILURES list and summarised in docs/conformance/shacl.md.

## `sparql/` section (added 2026-09-10)

- Source: https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/sparql`)
- Commit: 9c863967bceaef1a87c24e4dd761eda763823120 (vendored 2026-09-10)
- Content: SPARQL-based constraints (`node/`, `property/`), custom constraint
  components with `sh:validator` / `sh:nodeValidator` / `sh:propertyValidator`
  and `sh:parameter` (`component/`), and pre-binding (`pre-binding/`,
  including the `unsupported-sparql-*` cases whose expected outcome is
  `sht:Failure` — the validator must reject the shapes graph).
- Runner: the same `tests/w3c_shacl_conformance.rs`, second suite root; same
  comparison level and two-way ratchet.
