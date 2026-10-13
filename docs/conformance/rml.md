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

## Results (2026-10-03)

| Corpus | Cases | Pass | Known failures | Not run |
|---|---:|---:|---:|---:|
| RML-Core | 76 | 75 | 1 | 0 |
| RML-IO (sources) | 32 | 29 | 1 | 2 |
| Legacy RML (CSV, JSON, XML) | 117 | 112 | 5 | 0 |

The W3C R2RML cases publish no score (W3C test-suite policy). They run on
SQLite in the conformance job and on PostgreSQL 16 and MySQL 8.4 in the
live-database job; the known failures per database are below.

## Known failures

| Case | Why |
|---|---|
| R2RMLTC0002f (SQLite, PostgreSQL, MySQL) | SQL 2008 folds the regular identifier `Name` to `NAME`, which the delimited column `"Name"` is not, so the case expects an error. This engine matches a regular identifier as written, against the columns the database reports, because folding — to upper case by the standard, to lower case by PostgreSQL — would break mappings that name mixed-case columns without quotes. The suite itself lists the case as MySQL non-compliance. |
| R2RMLTC0018a (SQLite, MySQL) | Neither database returns a `CHAR(15)` value padded to its length (MySQL only under the deprecated `PAD_CHAR_TO_FULL_LENGTH` mode); the case expects the padding. PostgreSQL passes it. |
| RMLTC0027b-JSON | `rml:UnsafeIRI` writes template values unencoded, and the expected output holds an IRI with a space in it, which RDF does not allow; the store will not make one, and the run reports a data error. |
| RMLSTC0009a | The manifest says an error is expected; the suite's own description says none is, and the case ships the output of reading the quoted CSV header as RFC 4180 does, which is what this engine produces. |
| RMLTC0002c-JSON, RMLTC0002c-XML | The reference names a member no record has. The RML-IO registry makes such a reference NULL, not an error, and this engine follows it; the legacy suite expected an error. |
| RMLTC0007h-CSV, -JSON, -XML | A graph named by a literal is a non-conforming mapping (R2RML §7.4, RML-Core) and is refused; the legacy suite expected it to be ignored. |

## Not run

`RMLSTC0003` (a SPARQL endpoint as the source) and `RMLSTC0006a` (a database
described with D2RQ inside the mapping): this engine maps endpoints and
databases through registered datasources. `RMLTC0002g-JSON` of the legacy
suite has no row in its `metadata.csv`, so no expectation, and is not run.

The legacy cases run the way this engine's opt-in lenient mode does
(`on_data_error=skip`), because their expected outputs are those of a
processor that leaves a data error out of the output instead of failing the
run; the RML-Core and RML-IO cases run with the default, which aborts.
