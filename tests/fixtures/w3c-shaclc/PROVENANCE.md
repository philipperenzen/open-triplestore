# SHACL Compact Syntax test cases — vendored copy

- Source: https://github.com/w3c/data-shapes (`shacl-compact-syntax/tests/valid`),
  the test cases of the SHACL Compact Syntax Community Group report
  (https://w3c.github.io/shacl/shacl-compact-syntax/).
- Commit: 976ed12ad3491e1ad9725f2e4ad8b1047ff893d8 (vendored 2026-10-03). The
  files were last changed upstream on 2017-05-26 (b1a662c).
- Content: 32 pairs. Each `<name>.shaclc` must parse to a graph isomorphic to
  `<name>.ttl` (the report: "Parsing the .shaclc file must produce a graph
  that is isomorphic to the .ttl file. The test cases are not normative.").
- Runner: `tests/w3c_shaclc_conformance.rs`, a two-way ratchet with a
  `KNOWN_FAILURES` list. It parses every `.shaclc`, compares the graph with the
  `.ttl`, and also serializes the expected graph back to SHACL-C and checks the
  round trip. A document without a `BASE` is parsed with the initial base IRI
  `urn:x-base:default`, the one `empty.ttl` expects (the report leaves the
  parser's "optional base URI" to the caller).

## Licence, copyright and changes

- Licence: `LICENSE.md` is the w3c/data-shapes root `LICENSE.md` at the commit
  above, unchanged. It licenses every document in that repository, these test
  files included, under the W3C Software and Document License. Its link
  (http://www.w3.org/Consortium/Legal/copyright-software) resolves to the 2023
  version, whose full text the licence requires with every copy:
  `LICENSE-W3C-Software-and-Document-2023.txt`, verbatim from
  https://www.w3.org/copyright/software-license-2023/ (the same file as in
  `tests/fixtures/w3c-shacl/`). The files carry no test-suite licence
  statement, so the W3C Test Suite License / 3-clause BSD pair does not apply.
  The same files are also in the archived Community Group repository
  w3c/shacl (`shacl-compact-syntax/tests/valid`, commit
  e28389980cab29f0bb63f83a4037a6cbef52e3ba), which has no licence file; this
  copy is taken from w3c/data-shapes because that repository states the
  licence. Not covered by this project's AGPL-3.0 + Commons Clause licence.
- Authors: per the upstream git history of these paths, Holger Knublauch
  (2017), an editor of the report. No test file carries its own notice.
- Notice of changes, in the form the licence gives: This software or document
  includes material copied from SHACL Compact Syntax test cases,
  https://github.com/w3c/data-shapes/tree/976ed12ad3491e1ad9725f2e4ad8b1047ff893d8/shacl-compact-syntax/tests/valid.
  Copyright © 2017 World Wide Web Consortium.
  https://www.w3.org/copyright/software-license-2023/
- Changes: line endings only. This repository stores text with LF
  (`.gitattributes`), so the carriage returns of the 16 files upstream stores
  with CRLF were removed; every other byte is unchanged, and the result is
  byte-identical to the w3c/shacl copy named above. The 48 other files are
  byte-identical to upstream. Nothing was added to or removed from the
  directory.
