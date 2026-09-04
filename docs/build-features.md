# Build Features

Open Triplestore has optional Cargo features. This page says, per feature,
whether it is in the default `full` set (and therefore in a plain `cargo build`
and in the published Docker image, which builds with `--features full`), and
which CI pipeline compiles it. A feature that no pipeline compiles can break
without anyone noticing; a feature outside `full` is absent from the image no
matter what the docs say about its knobs.

| Feature | What it enables | In `full` / image | Compiled in CI |
|---|---|---|---|
| `rdf-12` | RDF 1.2 triple terms and SPARQL 1.2 accessor functions | yes | GitHub, GitLab |
| `rdfs-entailment` | RDFS materialisation | yes (via the OWL features) | GitHub, GitLab |
| `owl2-rl`, `owl2-el`, `owl2-ql`, `owl2-dl` | OWL 2 profile reasoners; `owl2-dl` adds the DL extension rules and the external-reasoner bridge | yes | GitHub, GitLab |
| `text-search`, `vocab-search` | Tantivy full-text index; vocabulary index | yes | GitHub, GitLab |
| `ldp` | Linked Data Platform 1.0 at `/ldp/` | yes | GitHub, GitLab |
| `shex`, `swrl` | ShEx validation; SWRL rule execution (both graded *Partial*, see [standards](standards.md)) | yes | GitHub, GitLab |
| `geometry3d` | 3D geometry (parry3d) for the viewer and OGC API endpoints | yes | GitHub, GitLab |
| `sfcgal3d` | SFCGAL-backed 3D operations (needs native SFCGAL ≥ 2.0) | **no** | GitLab only (`--all-features`); not in the image |
| `backup-encrypt` | age-encrypted backups (`BACKUP_ENCRYPT`) | yes | GitHub, GitLab |
| `alerting` | Ops alert dispatch (`ALERT_*`) | yes | GitHub, GitLab |
| `asset-pdf`, `asset-exif`, `asset-media`, `asset-archive`, `asset-spreadsheet`, `asset-thumbnail`, `asset-clamav` | Asset metadata extraction, thumbnails, ClamAV scanning | yes | GitHub, GitLab |
| `saml` | SAML 2.0 SSO — **experimental**, known non-working ACS path ([auth](auth.md)) | **no** | GitHub (explicit `saml` in the feature list), GitLab |
| `plugin-hello`, `plugin-accounts-dashboard` | Example / accounts-dashboard plugins mounted at `/ext` | **no** | GitHub, GitLab |
| `test-utils` | Test-only helpers | no | GitHub, GitLab (tests) |

Notes:

- `default = ["full"]`, so `cargo build --release` produces the same feature set
  as the image. Before this default existed, a plain build produced a binary
  with none of the optional standards compiled in.
- GitHub CI compiles `full,saml,test-utils,backup-encrypt,alerting,plugin-hello,plugin-accounts-dashboard`
  and, separately, `--no-default-features`; GitLab compiles `--all-features`
  (the only pipeline that builds `sfcgal3d`, which needs `libsfcgal-dev`).
- The conformance table in [standards](standards.md) is generated from the test
  suites, so feature claims and test coverage are checked together in CI.
