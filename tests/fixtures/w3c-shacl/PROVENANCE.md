# W3C SHACL test suite — vendored copy

- Source: https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/core`)
- Commit: b6e73695d6196f33d7ce3ba47094a10fbc298e65 (vendored 2026-06-10)
- License: W3C Software and Document License — see "Licence, copyright and
  changes" below.
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

## Licence, copyright and changes

- Licence: `LICENSE.md` is the w3c/data-shapes root `LICENSE.md`, unchanged
  (identical at both commits above). It licenses every document in that
  repository, these tests included, under the W3C Software and Document
  License. Its link (http://www.w3.org/Consortium/Legal/copyright-software)
  now resolves to the 2023 version, whose full text the licence requires with
  every copy: it is in `LICENSE-W3C-Software-and-Document-2023.txt`, verbatim
  from https://www.w3.org/copyright/software-license-2023/. The suite does not
  carry W3C's test-suite licence statement, so the W3C Test Suite License /
  3-clause BSD pair does not apply. Not covered by this project's AGPL-3.0 +
  Commons Clause licence.
- Authors: per the upstream git history of these paths, Holger Knublauch,
  Peter F. Patel-Schneider and Ashley Sommer (2017–2019); upstream's
  `LICENSE.md` states that the repository's documents are licensed by their
  contributors. No test file carries its own notice.
- Notice of changes, in the form the licence gives: This software or document
  includes material copied from SHACL Test Suite,
  https://github.com/w3c/data-shapes/tree/b6e73695d6196f33d7ce3ba47094a10fbc298e65/data-shapes-test-suite/tests/core
  and
  https://github.com/w3c/data-shapes/tree/9c863967bceaef1a87c24e4dd761eda763823120/data-shapes-test-suite/tests/sparql.
  Copyright © 2017–2019 World Wide Web Consortium.
  https://www.w3.org/copyright/software-license-2023/
- Changes: line endings only. This repository stores `*.ttl` with LF
  (`.gitattributes`), so the carriage returns of the 72 `core/` and 10
  `sparql/` files that upstream stores with CRLF or mixed line endings were
  removed; every other byte is unchanged, and the remaining 49 `core/` and 18
  `sparql/` files are byte-identical. Only `core/` and `sparql/` were copied:
  the suite's root `tests/manifest.ttl`, which only includes those two
  manifests, is not vendored, and nothing was added inside either section.
