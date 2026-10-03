# DCAT-AP-NL 3 SHACL shapes — fetched, not vendored

`fetch.sh` downloads Geonovum's DCAT-AP-NL 3 shapes into this folder from
https://github.com/Geonovum/DCAT-AP-NL30 (`shapes/`) at commit
`5106d18720fbe6ad56905f43e87fa9fe544104a7` (2026-09-25) and checks each file's
sha256:

| File | Constraints |
|---|---|
| `dcat-ap-nl-SHACL.ttl` | DCAT-AP-NL's mandatory properties and cardinalities |
| `dcat-ap-nl-SHACL-klassebereik.ttl` | class ranges |
| `dcat-ap-nl-SHACL-klassebereik-codelijsten.ttl` | class ranges of code-list values |
| `dcat-ap-nl-SHACL-aanbevolen.ttl` | recommended properties (`sh:Warning`) |

The repository's own `dcat-ap-SHACL.ttl` is byte-identical to the SEMIC
DCAT-AP 3.0.1 shapes vendored in `../semic-dcat-ap-3.0.1/`, which the runner
loads alongside these.

**Why fetched.** The DCAT-AP-NL specification is published under CC BY 4.0, but
the repository has no LICENSE file, so these files are not redistributed here:
they are fetched for a test run and git-ignored. DCAT-AP-NL 3.0.1 is still a
working version (open issues upstream); the pin keeps the run reproducible and
a sha256 mismatch fails it.

`tests/dcat_conformance.rs` validates the DCAT-AP-NL catalogue against the SEMIC
shapes plus these when they are present, and skips that part otherwise unless
`OTS_TEST_DCAT_AP_NL_REQUIRED=1` (set in CI after running `fetch.sh`).
