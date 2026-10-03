# Licence of the vendored W3C OWL 2 test cases

`approved/all.rdf` in this directory is an unmodified copy of
<https://www.w3.org/2009/11/owl-test/approved/all.rdf>, the export of the
test cases the W3C OWL Working Group approved for the OWL 2 Test Case
Repository (referenced as [OWL 2 Test Cases] by
[OWL 2 Conformance](https://www.w3.org/TR/owl2-conformance/) §3.3). It is
third-party material: Open Triplestore's AGPL-3.0 + Commons Clause licence
does not apply to it.

## Licence

The file carries no licence statement or copyright notice of its own, and the
OWL 2 Conformance specification states none for the repository. W3C's
dual test-suite licensing (W3C Test Suite License / W3C 3-clause BSD License,
<https://www.w3.org/copyright/test-suites-licenses/>) "does not affect existing
test suites until they are modified to include the new license", and this
suite never was. The W3C site's general rule therefore applies: "Unless
otherwise stated, documents on the W3C site are published under the W3C
document license" (<https://www.w3.org/copyright/intellectual-rights/>).

The file is redistributed under the **W3C Document License**
(<https://www.w3.org/copyright/document-license-2023/>; plain-text copy in
[`LICENSES/W3C-Document-License-2023.txt`](../../../LICENSES/W3C-Document-License-2023.txt)),
which permits copying and distributing the document, unmodified, for any
purpose, on condition that every copy carries a link to the original, the
original's copyright notice or, as here where none exists, a notice of the
form below, and the document's status.

- **Original:** <https://www.w3.org/2009/11/owl-test/approved/all.rdf>
  (Last-Modified 18 Nov 2009 15:49:36 GMT).
- **Notice:** Copyright © 2009 World Wide Web Consortium
  <https://www.w3.org/>. https://www.w3.org/copyright/document-license-2023/
- **Status:** the approved test cases of the OWL 2 Test Case Repository,
  maintained by the W3C OWL Working Group (closed); each test case names its
  creator in `test:creator`.

## How it is used

Only for development and regression testing: `tests/w3c_owl2_dl_manifests.rs`
reads the file, picks the approved OWL 2 DL / Direct Semantics cases and
converts each premise and conclusion to N-Triples in memory; the file itself
is never changed. W3C does not allow performance claims based on altered or
partial test suites, so this project publishes **no pass count, pass rate or
conformance claim** for these tests; `docs/conformance/owl2-dl.md` describes
the run and its known failures.
