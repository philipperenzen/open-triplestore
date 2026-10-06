# shexTest (ShEx test suite) — vendored copy

- Source: https://github.com/shexSpec/shexTest
- Commit: fc784a95e782db81a7236bf491d9091953070a1c (2026-09-24, vendored 2026-10-03)
- Licence: the W3C Software and Document License (2015 version) — see
  "Licence, copyright and changes" below.
- Runner: `tests/shextest_conformance.rs`. Its results are development and
  regression results, not a conformance claim.

## What is here

| Directory | Content | Files |
|---|---|---|
| `validation/` | `manifest.jsonld` / `manifest.ttl` (`sht:ValidationTest`, `sht:ValidationFailure`), data (`.ttl`), shape maps and their expected results (`.json`), a few schemas | 333 |
| `schemas/` | every schema in ShExC (`.shex`), ShExJ (`.json`) and mostly ShExR (`.ttl`); `manifest.jsonld` / `manifest.ttl` (`sht:RepresentationTest`); external semantic-action code (`.semact`) and external shapes (`.shextern`) | 1483 |
| `negativeSyntax/` | ShExC documents the grammar rejects (`sht:NegativeSyntax`) | 107 |
| `negativeStructure/` | grammatical ShExC documents that break a schema requirement (`sht:NegativeStructure`) | 21 |
| `context.jsonld` | the JSON-LD context the manifests reference (`../context.jsonld`) | 1 |
| `doc/ShExR.{shex,json,ttl}` | the ShExR schema, which the `ShExR` representation test reads | 3 |

The runner reads the JSON-LD manifests as plain JSON. Tests tagged with a ShEx
2.next trait (`Extends`, `MultiExtends`, `ExtendsDiamond`, `Abstract`) are
skipped: they exercise the 2.next draft (EXTENDS / ABSTRACT), which ShEx 2.1
does not include. Six representation tests of 2.next schemas carry no such
trait upstream; the runner lists them by name (`NEXT_UNTAGGED`) and checks
that each uses a 2.next keyword.

## Licence, copyright and changes

- Licence: `LICENSE` is the shexTest root `LICENSE` at the commit above,
  unchanged (added upstream in ae511cf, 2021-05-31). Its text is the W3C
  Software and Document License, 2015 version (also in
  `LICENSES/W3C-Software-and-Document-License-2015.txt`). The npm package
  metadata (`package.json`) says MIT; the `LICENSE` file is the operative
  notice for these files and is the one followed here. The suite does not
  carry W3C's test-suite licence statement, so the W3C Test Suite License /
  3-clause BSD pair does not apply, and the licence sets no condition on
  publishing results. Not covered by this project's AGPL-3.0 + Commons Clause
  licence.
- Authors: per the upstream git history of these paths, Eric Prud'hommeaux,
  Gregg Kellogg, Iovka Boneva, Harold Solbrig, Jose Emilio Labra Gayo and
  Jérémie Dusart (2015–2026), working in the W3C Shape Expressions Community
  Group. No test file carries its own notice.
- Notice of changes, in the form the licence gives: This software or document
  includes material copied from shexTest,
  https://github.com/shexSpec/shexTest/tree/fc784a95e782db81a7236bf491d9091953070a1c.
  Copyright © 2015–2026 the shexTest contributors.
  https://www.w3.org/Consortium/Legal/2015/copyright-software-and-document
- Changes: none to any copied file — every file here is byte-identical to
  upstream (`.gitattributes` exempts this directory from line-ending
  normalisation; `validation/Is1_Ip1_L_with_REGEXP_escapes_bare.ttl` holds a
  carriage return inside a literal). Only the four directories above,
  `context.jsonld` and the three `doc/ShExR.*` files were copied, and from them the build files (`Makefile`),
  scripts (`*.js`) and spreadsheets (`*.xlsx`, `*.ods`) were left out. This
  `PROVENANCE.md` is the only file added.
