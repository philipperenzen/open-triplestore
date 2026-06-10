# OGC GeoSPARQL 1.1 SHACL validator — vendored copy

- Source: https://github.com/opengeospatial/ogc-geosparql
  (`vocabularies/validator.ttl` + `examples/shacl/*.ttl`)
- Commit: 523098e714bb077a800f048be2940942b66310c8 (vendored 2026-06-10)
- License/rights: (c) Open Geospatial Consortium, https://www.ogc.org/license
  (validator is informative as of GeoSPARQL 1.1)
- Runner: `tests/ogc_geosparql_shacl_roundtrip.rs` — validates (1) the OGC's own
  Sxx-valid / Sxx-invalid example files and (2) the Waalbrug dataset against the
  official validator shapes, using this repo's native SHACL engine.
