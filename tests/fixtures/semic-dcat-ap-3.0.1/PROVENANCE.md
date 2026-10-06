# SEMIC DCAT-AP 3.0.1 SHACL shapes — vendored copy

- Source: https://github.com/SEMICeu/DCAT-AP (`releases/3.0.1/shacl/`)
- Commit: 4470b8ea907b7eb9ec55ab22befd1ecd2210ff32 (vendored 2026-10-03; the
  two files last changed upstream in 729eddfc176d0afee5850ade6528f96f72579412,
  2025-07-10)
- Files, unmodified (sha256):
  - `dcat-ap-SHACL.ttl` — `990d3e42721de6a4be8cc338a7171559f195e62dea89c0b56531356b78cc026f`
    (the mandatory-property, cardinality, node-kind and class constraints)
  - `ranges.ttl` — `a6eed0fae8d0f5ca977fe2098ca12081ac60b0efe1ddce802d5c08e49505ebcc`
    (the range constraints)
- Licence: Creative Commons Attribution 4.0 International, © European Union —
  see `LICENSE.md`.
- Changes: none; both files are byte-identical to upstream at the commit above.
- Upstream defect worked around by the runners, not in the files: taken
  together, the two files link five property shapes that neither defines (no
  `sh:path`, no triple at all) —
  `dcat:DatasetShape/4918ff7a6c1c4b0eea6403dca4b992b87ee1f4ab`,
  `dcat:DatasetShape/95c69c99a1e3ade043911b51b942f206dea0e68d`,
  `dcat:DistributionShape/653804840386e33525b3d39d205c174780be414b`,
  `dcat:DistributionShape/a07d6e7a0a1790b89a1ce7ff602cbbd9ea835282` and
  `dcat:RelationshipShape/b7aa98e1befa5130659568aa62e7f38575dc17c1` (all under
  `https://semiceu.github.io/DCAT-AP/releases/3.0.1#`). A pathless property
  shape makes a shapes graph ill-formed, and this repo's engine refuses such a
  graph whole, so the runners drop those five `sh:property` links in memory
  before validating; they constrain nothing. `tests/dcat_conformance.rs`
  asserts the count, so an upstream change shows up.
- Runner: `tests/dcat_conformance.rs` validates the catalogue, built per
  application profile over a fixture that exercises every branch of the
  generator, against these shapes with this repo's SHACL engine and asserts no
  violation; `tests/dcat_ap_http.rs` does the same over the served
  `/.well-known/void`. Passing them is the project's own check against the
  published shapes, not a certification by SEMIC or the European Commission.
