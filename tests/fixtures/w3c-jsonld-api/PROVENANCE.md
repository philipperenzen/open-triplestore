# W3C JSON-LD 1.1 API test suite — vendored subset

- Source: https://github.com/w3c/json-ld-api (`tests/`)
- Commit: ffdb326121ea89b7b8280e76a5caea923834bcef (2026-08-12; vendored 2026-10-03)
- Content: the `toRdf` and `fromRdf` sections — `toRdf/`, `fromRdf/`,
  `toRdf-manifest.jsonld` and `fromRdf-manifest.jsonld` — plus
  `expand/er56-in.jsonld`, the one input of another section the `toRdf`
  manifest names (entry `#ter56`). The other sections
  (compact, expand, flatten, html, remote-doc) and the suite's root manifest,
  its JSON-LD context and test vocabulary are not included.
- Runner: `tests/w3c_jsonld_api_manifests.rs` (see its header and
  `docs/conformance/jsonld.md`).
- Use: development and regression testing only. This is a subset of a W3C
  test suite, so no result is published for it
  (https://www.w3.org/copyright/test-suites-licenses/).

## Licence, copyright and changes

- Licence: `LICENSE.md` is the w3c/json-ld-api root `LICENSE.md`, unchanged.
  It licenses every document in that repository, these tests included, under
  the W3C Software and Document License
  (https://www.w3.org/copyright/software-license-2023/); full text in
  `LICENSE-W3C-Software-and-Document-2023.txt` here and in
  `LICENSES/W3C-Software-and-Document-License-2023.txt`.
- Copyright © 2010–2026 World Wide Web Consortium; authors per the upstream
  history (the JSON-LD Community Group and the JSON-LD Working Groups).
- Changes: none. Every vendored file is byte-identical to the commit above.
