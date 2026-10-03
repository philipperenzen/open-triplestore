# Build Features

Open Triplestore has optional Cargo features. This page says, per feature,
whether it is in the default `full` set (and therefore in a plain `cargo build`),
whether the published Docker image has it, and which CI pipeline compiles it.
The image builds with the Dockerfile's `CARGO_FEATURES`, which defaults to
`full,plugin-postgres,plugin-mysql,plugin-mssql`: everything in `full` plus the
three SQL connectors. A feature that no pipeline compiles can break without
anyone noticing; a feature in neither `full` nor that list is absent from the
image no matter what the docs say about its knobs.

| Feature | What it enables | In `full` / image | Compiled in CI |
|---|---|---|---|
| `rdf-12` | RDF 1.2 triple terms and SPARQL 1.2 accessor functions | yes | GitHub, GitLab |
| `rdfs-entailment` | RDFS materialisation | yes (via the OWL features) | GitHub, GitLab |
| `owl2-rl`, `owl2-el`, `owl2-ql`, `owl2-dl` | OWL 2 profile reasoners; `owl2-dl` adds the DL backends (native rules, Konclude, the client for the bundled OWL API + HermiT reasoner sidecar), the OWL 2 DL profile check and `POST /api/reasoning/check` | yes | GitHub, GitLab |
| `text-search`, `vocab-search` | Tantivy full-text index; vocabulary index | yes | GitHub, GitLab |
| `ldp` | Linked Data Platform 1.0 at `/ldp/` | yes | GitHub, GitLab |
| `shex`, `swrl` | ShEx validation; SWRL rule execution (both graded *Partial*, see [standards](standards.md)) | yes | GitHub, GitLab |
| `geometry3d` | 3D geometry (parry3d) for the viewer and OGC API endpoints | yes | GitHub, GitLab |
| `sfcgal3d` | SFCGAL-backed 3D operations (needs native SFCGAL ≥ 2.0) | **no** | GitLab only (`--all-features`); not in the image |
| `backup-encrypt` | age-encrypted backups (`BACKUP_ENCRYPT`) | yes | GitHub, GitLab |
| `alerting` | Ops alert dispatch (`ALERT_*`) | yes | GitHub, GitLab |
| `asset-pdf`, `asset-exif`, `asset-media`, `asset-archive`, `asset-spreadsheet`, `asset-thumbnail`, `asset-clamav` | Asset metadata extraction, thumbnails, ClamAV scanning | yes | GitHub, GitLab |
| `saml` | SAML 2.0 SSO, SP-initiated — **experimental**, not yet verified against a real IdP ([auth](auth.md#saml-20)) | **no** | GitHub (explicit `saml` in the feature list), GitLab |
| `plugin-postgres`, `plugin-mysql`, `plugin-mssql` | SQL datasource connectors for PostgreSQL, MySQL / MariaDB and SQL Server ([sources](sources.md)); pure Rust over rustls, no system libraries | not in `full`; **yes** in the image | GitHub (backend job; `live-sources` runs each against a live server), GitLab |
| `plugin-hello`, `plugin-accounts-dashboard` | Example / accounts-dashboard plugins mounted at `/ext` | **no** | GitHub, GitLab |
| `test-utils` | Test-only helpers | no | GitHub, GitLab (tests) |

Notes:

- `default = ["full"]`, so `cargo build --release` produces the image's feature
  set without the SQL connectors; add
  `--features full,plugin-postgres,plugin-mysql,plugin-mssql` to match the image
  exactly. Before this default existed, a plain build produced a binary with
  none of the optional standards compiled in.
- The connectors stay out of `full` so a source build carries only the drivers
  its operator asks for. An image without them builds with
  `docker build --build-arg CARGO_FEATURES=full .`; an image with more plugins
  repeats the connector list in its `CARGO_FEATURES`, since the argument
  replaces the default rather than adding to it.
- GitHub CI compiles `full,saml,test-utils,backup-encrypt,alerting,plugin-hello,plugin-accounts-dashboard,plugin-postgres,plugin-mysql,plugin-mssql`
  and, separately, `--no-default-features`; GitLab compiles `--all-features`
  (the only pipeline that builds `sfcgal3d`, which needs `libsfcgal-dev`).
- The conformance table in [standards](standards.md) is generated from the test
  suites, so feature claims and test coverage are checked together in CI.
