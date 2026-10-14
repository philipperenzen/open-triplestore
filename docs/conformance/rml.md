# RML and R2RML — results on the test corpora

Four corpora run in CI. Two are vendored, with their licence and provenance
beside them; one is vendored for the legacy vocabulary; the W3C R2RML test
cases are fetched at a pinned commit and never committed.

| Corpus | Runner | Source | Licence |
|---|---|---|---|
| RML-Core test cases | [`tests/rml_core_conformance.rs`](../../tests/rml_core_conformance.rs) | [`tests/fixtures/rml-core/`](../../tests/fixtures/rml-core/PROVENANCE.md), kg-construct/rml-core @ `82ab28d4` | CC BY-SA 4.0 (the stricter of the two the repository states) |
| RML-IO source test cases | [`tests/rml_io_conformance.rs`](../../tests/rml_io_conformance.rs) | [`tests/fixtures/rml-io/`](../../tests/fixtures/rml-io/PROVENANCE.md), kg-construct/rml-io @ `c0c5902b` | CC BY-SA 4.0 (likewise) |
| Legacy RML test cases (CSV, JSON, XML) | [`tests/rml_legacy_conformance.rs`](../../tests/rml_legacy_conformance.rs) | [`tests/fixtures/rml-legacy/`](../../tests/fixtures/rml-legacy/PROVENANCE.md), kg-construct/rml-test-cases @ `803dd3ec` | CC BY 4.0 |
| W3C R2RML test cases | [`tests/w3c_r2rml_conformance.rs`](../../tests/w3c_r2rml_conformance.rs) | fetched by `scripts/fetch-w3c-r2rml-tests.sh`, kg-construct/r2rml-test-cases-support @ `97609880`, sha256-checked | W3C test-suite licences; not redistributed, no score published |

The counts below are development and regression results on the vendored
corpora, compared by isomorphism of the output dataset (or, for a case that
expects an error, by the mapping being refused or the run failing). They are
not a claim of conformance to any specification, and the Knowledge Graph
Construction Community Group has not reviewed them. Every runner is a two-way
ratchet: a case that fails without being listed fails CI, and so does a listed
case that passes.

## Results (2026-10-03; known failures re-checked 2026-10-06)

| Corpus | Cases | Pass | Known failures | Not run |
|---|---:|---:|---:|---:|
| RML-Core | 76 | 75 | 1 | 0 |
| RML-IO (sources) | 32 | 29 | 1 | 2 |
| Legacy RML (CSV, JSON, XML) | 117 | 112 | 5 | 0 |

The W3C R2RML cases publish no score (W3C test-suite policy). They run on
SQLite in the conformance job and on PostgreSQL 16 and MySQL 8.4 in the
live-database job; the known failures per database are below.

## Known failures

Each is an engine limit or a test-suite defect, the two kinds the Full rubric
of [`docs/standards.md`](../standards.md) accepts, and each has its issue.

| Case | Kind | Why |
|---|---|---|
| R2RMLTC0002f (SQLite, MySQL) | engine limit ([#511](https://github.com/philipperenzen/open-triplestore/issues/511)) | The case names the delimited column `"Name"` with the regular identifier `Name`, which SQL 2008 folds to `NAME`, and expects an error. Column and table names are resolved as SQL identifiers by each engine's rules (R2RML §5; [sources.md](../sources.md#relational-logical-sources)), so it passes on PostgreSQL. SQLite and MySQL compare names without regard to case, delimited or not, and do not record which were declared delimited, so on them `Name` names `"Name"`. The suite's own harness lists the case as MySQL non-compliance. |
| R2RMLTC0018a (SQLite) | engine limit ([#512](https://github.com/philipperenzen/open-triplestore/issues/512)) | SQLite has no fixed-length `CHAR`, so `"Venus"` comes back without the padding to `CHAR(15)` the case expects. PostgreSQL pads, and so does MySQL since the connector sets `PAD_CHAR_TO_FULL_LENGTH` per session. |
| R2RMLTC0002d, R2RMLTC0003b, R2RMLTC0011a, R2RMLTC0014b, R2RMLTC0014c (PostgreSQL) | test-suite defect ([#513](https://github.com/philipperenzen/open-triplestore/issues/513)) | Each names a mixed-case delimited alias of its view (`"StudentId"`, `"FirstName"`, `"deptId"`) with a regular identifier, which R2RML §5 says is not the same name — the very error R2RMLTC0002f expects. PostgreSQL folds the regular name to lower case and the run is refused, naming the column to write delimited. They pass on SQLite and MySQL only because those compare without regard to case. |
| RMLTC0027b-JSON | test-suite defect ([#514](https://github.com/philipperenzen/open-triplestore/issues/514)) | `rml:UnsafeIRI` writes template values unencoded, and the expected output holds `<http://example.com/Person/Emily Smith>`, an IRI with a space in it. RML-Core's own term generation rules make that a data error, and RDF does not allow the IRI; the run reports the data error. |
| RMLSTC0009a | test-suite defect ([#515](https://github.com/philipperenzen/open-triplestore/issues/515)) | The manifest says an error is expected; the suite's own `descriptions.csv` says none is, and the case ships the output of reading the quoted CSV header as RFC 4180 does, which is what this engine produces. |
| RMLTC0002c-JSON, RMLTC0002c-XML | test-suite defect ([#516](https://github.com/philipperenzen/open-triplestore/issues/516)) | The reference names a member no record has. The RML-IO registry makes such a reference NULL, not an error, and this engine follows it; the legacy suite expected an error. |
| RMLTC0007h-CSV, -JSON, -XML | test-suite defect ([#516](https://github.com/philipperenzen/open-triplestore/issues/516)) | A graph named by a literal is a non-conforming mapping (R2RML §7.4, RML-Core) and is refused; the legacy suite expected it to be ignored. |

## Not run

`RMLSTC0003` (a SPARQL endpoint as the source) and `RMLSTC0006a` (a database
described with D2RQ inside the mapping): this engine maps endpoints and
databases through registered datasources. `RMLTC0002g-JSON` of the legacy
suite has no row in its `metadata.csv`, so no expectation, and is not run.

The legacy cases run the way this engine's opt-in lenient mode does
(`on_data_error=skip`), because their expected outputs are those of a
processor that leaves a data error out of the output instead of failing the
run; the RML-Core and RML-IO cases run with the default, which aborts.
