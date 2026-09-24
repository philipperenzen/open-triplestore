# OGC GeoSPARQL 1.1 SHACL validator — vendored copy

- Source: https://github.com/opengeospatial/ogc-geosparql
  (`vocabularies/validator.ttl` + `examples/shacl/*.ttl`)
- Commit: 523098e714bb077a800f048be2940942b66310c8 (vendored 2026-06-10)
- License/rights: © Open Geospatial Consortium; `validator.ttl` carries
  "(c) 2022 Open Geospatial Consortium" and names the OGC GeoSPARQL Standards
  Working Group as creator. Redistributed under the Apache License 2.0, which
  the upstream README's License section grants for the repository's software
  and data; full text in LICENSE-Apache-2.0.txt, details in LICENSE.md.
  `validator.ttl` also declares `dcterms:license` https://www.ogc.org/license,
  the OGC Document License Agreement. Not covered by this project's
  AGPL-3.0 + Commons Clause licence.
- Changes: none — all 49 files are byte-identical to upstream at the commit
  above. The 48 examples are every `.ttl` file in upstream `examples/shacl/`
  (upstream has no S05–S08 and no S20-invalid); that folder's `README.md` and
  `test_shapes.py` were not copied.
- Upstream has since moved: the RDF was removed from ogc-geosparql's master on
  2026-07-29 (commit c58fefcb33). The OGC's current copy of the validator is
  `resources/geosparql-swg/geosparql-1.1/validators/geo-validator.ttl` in
  https://github.com/opengeospatial/geosemantics-semantic-resources
  (Apache-2.0).
- Status: the validator is informative, not normative, as of GeoSPARQL 1.1.
- Runner: `tests/ogc_geosparql_shacl_roundtrip.rs` — validates (1) the OGC's own
  Sxx-valid / Sxx-invalid example files and (2) the Waalbrug dataset against the
  validator shapes, using this repo's native SHACL engine. Passing them is not
  an OGC compliance certification.
