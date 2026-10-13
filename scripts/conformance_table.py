#!/usr/bin/env python3
"""Generate the conformance table from the test suites themselves.

The hand-maintained tables in README.md and docs/standards.md drifted from the
code (112 SPARQL tests claimed vs 125 present, 84 GeoSPARQL vs 107, the W3C SHACL
corpus omitted entirely). This script counts the `#[test]` / `#[tokio::test]`
functions in each `tests/*.rs` suite — a count that matches what `cargo test`
runs, checked for every suite — plus the baselines the vendored W3C corpus
runners record, and writes the result between `<!-- conformance-table:start -->`
/ `:end -->` markers.

    scripts/conformance_table.py            # print the table
    scripts/conformance_table.py --write    # update README.md and docs/standards.md
    scripts/conformance_table.py --check    # exit 1 if either file is stale (CI)

The *basis* column is the honest part: only the rows marked **vendored** run
a published test corpus (today the W3C SPARQL 1.1 query/update and federation
sections, the W3C SPARQL 1.2 suite, the W3C RDF 1.2 (+ included RDF 1.1)
syntax suites, the W3C SHACL core/sparql sections, TopQuadrant's SHACL-AF tests,
the approved W3C OWL 2 DL test cases and the OGC GeoSPARQL validator shapes);
every other suite is hand-written and *derived from* the spec text.

Which corpus results are published is a licence question, not a style one, and
is decided per corpus when it is vendored (`CORPUS_RUNNERS`, `PUBLISH_SCORE`,
`UNSCORED_NOTES`). The current ones:

- The SPARQL 1.1 sections, the SPARQL 1.2 suite and the RDF 1.2 / RDF 1.1
  syntax suites come from W3C test suites, which W3C licenses under
  its 3-clause BSD licence for "software development, bug tracking, and other
  applications that do not require assertions of performance to the public",
  and a subset of such a suite "does not allow claims of performance and the use
  of the name W3C" without a special licence from W3C
  (https://www.w3.org/copyright/test-suites-licenses/). This project runs a
  subset, so its row says only that the corpus runs in CI as a regression
  ratchet: no case count, pass count, pass rate or floor. The runner keeps no
  pass count either, not even in a comment; its known-failure list and pass
  floor drive the ratchet in the test itself, and this script reads nothing
  from it.
- The OWL 2 test cases (the approved export of the OWL 2 Test Case
  Repository) carry no licence of their own, so the W3C Document License
  applies: verbatim copies only. The runner uses the OWL 2 DL / Direct
  Semantics cases and needs the reasoner sidecar, so it is a partial run and
  its row, like SPARQL's, publishes no numbers.
- The JSON-LD API toRdf and fromRdf sections are likewise a subset of a W3C
  test suite (w3c/json-ld-api, W3C Software and Document License); following
  W3C's test-suite policy for subsets, the row publishes no numbers either.
- The SHACL sections are under the W3C Software and Document License, which
  sets no such condition, so that row keeps its counts (`PUBLISH_SCORE`).
- The SHACL-AF tests are TopQuadrant's, under the Apache License 2.0, which
  sets no such condition either; the owner decided to publish their counts
  (2026-10-02). They are TopQuadrant's tests of its own engine, not a W3C
  suite, so a count is no claim of conformance to anything.
- The shexTest suite (ShEx Community Group) carries the W3C Software and
  Document License in its own LICENSE file, and is not a W3C test suite, so
  its row keeps its counts too.
- The OGC validator shapes are under the Apache License 2.0; only the OGC
  authorises compliance marks for its standards, so no row claims compliance.
- The buildingSMART IDS corpus is CC BY-ND 4.0 and is never committed (the
  runner downloads it and checks each file's SHA-256). Its results are
  development results, not a buildingSMART certification, so its row
  publishes no score either.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TESTS = ROOT / "tests"
START, END = "<!-- conformance-table:start -->", "<!-- conformance-table:end -->"
TARGETS = [ROOT / "README.md", ROOT / "docs" / "standards.md"]

# suite file stem -> (standard, basis)
SUITES: dict[str, tuple[str, str]] = {
    "w3c_sparql11_conformance": ("SPARQL 1.1 Query/Update", "spec-derived (+ cx01–cx15 high-complexity)"),
    "sparql12_conformance": ("SPARQL 1.2 / RDF 1.2", "spec-derived"),
    "sparql_functions_conformance": ("SPARQL 1.1 functions", "spec-derived"),
    "sparqloscope_conformance": ("SPARQL engine coverage (sparqloscope)", "sparqloscope-derived"),
    "sparql_benchmarks": ("SP2B / BSBM query shapes", "benchmark-derived"),
    "rdf11_conformance": ("RDF 1.1 formats", "spec-derived"),
    "api_protocol_conformance": ("SPARQL 1.1 Protocol / Graph Store", "spec-derived"),
    "geosparql_conformance": ("GeoSPARQL 1.1", "spec-derived"),
    "ogc_geosparql_shacl_roundtrip": ("OGC GeoSPARQL 1.1 validator shapes", "**vendored OGC corpus** (unmodified)"),
    "rdfs_conformance": ("RDFS entailment", "spec-derived"),
    "owl2_rl_conformance": ("OWL 2 RL", "spec-derived"),
    "owl2_el_conformance": ("OWL 2 EL", "spec-derived"),
    "owl2_ql_conformance": ("OWL 2 QL", "spec-derived"),
    "owl2_dl_conformance": ("OWL 2 DL", "spec-derived (+ live tests against the reasoner sidecar)"),
    "w3c_owl2_dl_manifests": ("OWL 2 DL", "**vendored W3C test cases** (approved OWL 2 DL / Direct Semantics cases of the OWL 2 Test Case Repository, unmodified; manifest-driven, against the reasoner sidecar)"),
    "shacl_conformance": ("SHACL Core", "spec-derived"),
    "w3c_shacl_conformance": ("SHACL Core", "**vendored W3C corpus** (core + sparql sections, manifest-driven, full report equality)"),
    "w3c_sparql11_manifests": ("SPARQL 1.1 Query/Update", "**vendored W3C test-suite subset** (query + update sections of w3c/rdf-tests, unmodified; manifest-driven)"),
    "w3c_jsonld_api_manifests": ("JSON-LD 1.1 API", "**vendored W3C test-suite subset** (toRdf + fromRdf sections of w3c/json-ld-api, unmodified; manifest-driven)"),
    "w3c_sparql11_entailment_manifests": ("SPARQL 1.1 Entailment Regimes", "**vendored W3C test-suite subset** (entailment section of w3c/rdf-tests, unmodified; manifest-driven)"),
    "w3c_rdf_mt_manifests": ("RDF 1.1 Semantics (RDF/RDFS entailment)", "**vendored W3C test-suite subset** (rdf-mt section of w3c/rdf-tests, unmodified; manifest-driven)"),
    "w3c_owl2_rl_manifests": ("OWL 2 RL", "**vendored W3C test cases** (approved OWL 2 cases of the RL profile, unmodified; manifest-driven)"),
    "w3c_sparql11_federation": ("SPARQL 1.1 Federated Query", "**vendored W3C test-suite subset** (`service/` + `syntax-fed/` sections of w3c/rdf-tests, unmodified; manifest-driven, local endpoints)"),
    "w3c_sparql12_manifests": ("SPARQL 1.2", "**vendored W3C test-suite subset** (`sparql/sparql12` of w3c/rdf-tests, unmodified; manifest-driven, engine and mirror paths)"),
    "w3c_rdf12_manifests": ("RDF 1.2 formats", "**vendored W3C test-suite subset** (N-Triples, N-Quads, Turtle, TriG, RDF/XML suites of `rdf/rdf12` + the `rdf/rdf11` suites they include, unmodified; manifest-driven)"),
    "shacl_rules_conformance": ("SHACL-AF rules", "spec-derived"),
    "shacl_af_corpus": ("SHACL Advanced Features", "**vendored TopQuadrant corpus** (expression, function, rule and target tests of TopQuadrant/shacl, unmodified; dash-driven, full report equality)"),
    "shaclc_conformance": ("SHACL Compact Syntax", "spec-derived"),
    "w3c_shaclc_conformance": ("SHACL Compact Syntax", "**vendored W3C CG test cases** (SHACL-C report, line endings normalised; parse + round trip)"),
    "shex_conformance": ("ShEx", "spec-derived"),
    "shextest_conformance": ("ShEx 2.1", "**vendored shexTest corpus** (validation, representation, negative syntax/structure; manifest-driven)"),
    "swrl_conformance": ("SWRL", "spec-derived"),
    "ldp_conformance": ("LDP 1.0 (store level)", "spec-derived"),
    "ldp_http_conformance": ("LDP 1.0 (HTTP)", "spec-derived"),
    "dcat_conformance": ("DCAT 3 / DCAT-AP 3 / VoID", "spec-derived + **vendored SEMIC DCAT-AP 3.0.1 shapes** (unmodified; Geonovum DCAT-AP-NL 3 shapes fetched in CI)"),
    "ldes_conformance": ("LDES 1.0 / TREE", "spec-derived"),
    "rml_conformance": ("RML / R2RML", "spec-derived"),
    "rdf_patch_conformance": ("RDF Patch (RDF Delta)", "spec-derived"),
    "rml_core_conformance": ("RML-Core", "**vendored KG-Construct CG corpus** (the RML-Core test cases, unmodified; manifest-driven)"),
    "rml_io_conformance": ("RML-IO (sources)", "**vendored KG-Construct CG corpus** (the RML-IO source test cases, unmodified; manifest-driven)"),
    "rml_legacy_conformance": ("RML (legacy vocabulary)", "**vendored RML.io corpus** (the CSV, JSON and XML cases of rml-test-cases, unmodified)"),
    "w3c_r2rml_conformance": ("R2RML", "**fetched W3C test cases** (pinned commit + sha256, not vendored; SQLite here, PostgreSQL and MySQL in the live-database job)"),
    "standards_conformance": ("Cross-standard HTTP smoke", "spec-derived"),
    "buildingsmart_ids_conformance": ("buildingSMART IDS 1.0", "**buildingSMART IDS test corpus**, fetched at a pinned commit and SHA-256 checked (not in the repository)"),
}

TEST_ATTR = re.compile(r"^\s*#\[(?:tokio::)?test(?:\(|\])", re.M)
IGNORE_ATTR = re.compile(r"^\s*#\[ignore", re.M)


def count(path: Path) -> tuple[int, int]:
    text = path.read_text(encoding="utf-8")
    return len(TEST_ATTR.findall(text)), len(IGNORE_ATTR.findall(text))


# Manifest-driven runners and the pass floor they assert. A runner whose score
# is published (PUBLISH_SCORE) also records an `Empirical baseline` comment
# above its KNOWN_FAILURES list, which this script reads and cross-checks.
CORPUS_RUNNERS = {
    "shacl_af_corpus": 9,
    "w3c_shacl_conformance": 90,
    "w3c_shaclc_conformance": 32,
    "shextest_conformance": 1795,
    "w3c_sparql11_manifests": 450,
    "w3c_sparql11_federation": 9,
    "w3c_owl2_dl_manifests": 235,
    "w3c_sparql12_manifests": 250,
    "w3c_rdf12_manifests": 1250,
    "w3c_jsonld_api_manifests": 470,
    "w3c_sparql11_entailment_manifests": 37,
    "w3c_rdf_mt_manifests": 41,
    "w3c_owl2_rl_manifests": 45,
    "rml_core_conformance": 70,
    "rml_io_conformance": 25,
    "rml_legacy_conformance": 100,
    "buildingsmart_ids_conformance": 0,
}

# Runners whose score may be published (see the module docstring). A runner
# missing here is still checked, but its row carries no numbers: the W3C SPARQL
# 1.1 and JSON-LD API sections and the OWL 2 DL cases are partial runs of W3C test suites, on
# which W3C allows no public performance claims.
PUBLISH_SCORE = {
    "shacl_af_corpus",
    "shextest_conformance",
    "w3c_shacl_conformance",
    "w3c_shaclc_conformance",
    # The RML corpora are Creative Commons material (CC BY 4.0 and, treated as
    # the stricter of two stated licences, CC BY-SA 4.0), which set no
    # condition on reporting results.
    "rml_core_conformance",
    "rml_io_conformance",
    "rml_legacy_conformance",
}

# The note for each corpus runner whose score is not published. Every
# CORPUS_RUNNERS entry outside PUBLISH_SCORE needs one: it says why no score is
# given and where the known gaps are tracked.
UNSCORED_NOTES = {
    "w3c_jsonld_api_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C test-suite policy); known gaps in `docs/conformance/jsonld.md`"
    ),
    "w3c_sparql11_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C test-suite policy); known gaps in `docs/conformance/sparql11.md`"
    ),
    "w3c_sparql11_federation": (
        "runs in CI as a development and regression ratchet against local endpoints; "
        "no score is published (W3C test-suite policy); see `docs/conformance/sparql11.md` §Federation"
    ),
    "w3c_owl2_dl_manifests": (
        "runs in CI against the reasoner sidecar as a development and regression ratchet; "
        "no score is published (W3C licence: no performance claims on a partial run); "
        "known gaps in `docs/conformance/owl2-dl.md`"
    ),
    "w3c_sparql12_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C test-suite policy); known gaps in `docs/conformance/sparql12.md`"
    ),
    "w3c_rdf12_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C test-suite policy); known gaps in `docs/conformance/rdf12.md`"
    ),
    "w3c_sparql11_entailment_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C test-suite policy); known gaps in `docs/conformance/entailment.md`"
    ),
    "w3c_rdf_mt_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C test-suite policy); known gaps in `docs/conformance/entailment.md`"
    ),
    "w3c_owl2_rl_manifests": (
        "runs in CI as a development and regression ratchet; no score is published "
        "(W3C licence: no performance claims on a partial run); known gaps in "
        "`docs/conformance/owl2-rl.md`"
    ),
    "buildingsmart_ids_conformance": (
        "runs in CI as a development and regression ratchet; no score is published, "
        "and the results are not a buildingSMART certification; known gaps in "
        "`docs/conformance/ids.md`"
    ),
}

# Runners over a corpus that is fetched at CI time rather than vendored, and
# whose score is not published either: the note their row carries.
FETCHED_RUNNERS = {
    "w3c_r2rml_conformance": (
        "runs in CI as a development and regression ratchet over the cases fetched by "
        "`scripts/fetch-w3c-r2rml-tests.sh`; no score is published (W3C test-suite policy)"
    ),
}


def corpus(stem: str) -> tuple[int, int, int, int, int, int]:
    """(cases, pass, known failures, runner-side skips, optional-unsupported,
    non-spec expectations) from the runner's own recorded baseline
    (`Empirical baseline: N pass / N known-fail / N aux skips [/ N optional
    unsupported] [/ N non-spec expectations]` in tests/<stem>.rs) and its
    KNOWN_FAILURES, OPTIONAL_UNSUPPORTED and NON_SPEC_EXPECTATIONS lists. File
    counts are not used: the corpus directories hold shared/aux files beyond
    the cases.

    Optional-unsupported cases test a feature the specification makes optional
    and requires a processor without it to report as a failure; non-spec
    expectations are cases whose expected outcome rests on another engine's
    behaviour where the specification requires a failure. The runner passes
    both only when that failure is reported."""
    src = (TESTS / f"{stem}.rs").read_text(encoding="utf-8")
    m = re.search(
        r"Empirical baseline: (\d+) pass / (\d+) known-fail / (\d+) aux skips"
        r"(?: / (\d+) optional unsupported)?(?: / (\d+) non-spec expectations?)?",
        src,
    )
    if not m:
        raise SystemExit(f"{stem}.rs: baseline comment not found")
    passed, failed, skipped = (int(x) for x in m.groups()[:3])
    optional = int(m.group(4) or 0)
    non_spec = int(m.group(5) or 0)

    def entries(const: str) -> int:
        if f"const {const}" not in src:
            return 0
        block = src.split(f"const {const}", 1)[1].split("];", 1)[0]
        # An entry is a tuple whose first element is its key: `("key", …`,
        # with the key on the same line as `(` or the next (rustfmt).
        return len(re.findall(r'\(\s*"[^"]+"\s*,', block))

    known = entries("KNOWN_FAILURES")
    if known != failed:
        raise SystemExit(f"{stem}.rs: KNOWN_FAILURES has {known} entries but the baseline says {failed}")
    if entries("OPTIONAL_UNSUPPORTED") != optional:
        raise SystemExit(
            f"{stem}.rs: OPTIONAL_UNSUPPORTED has {entries('OPTIONAL_UNSUPPORTED')} entries but the baseline says {optional}"
        )
    if entries("NON_SPEC_EXPECTATIONS") != non_spec:
        raise SystemExit(
            f"{stem}.rs: NON_SPEC_EXPECTATIONS has {entries('NON_SPEC_EXPECTATIONS')} entries but the baseline says {non_spec}"
        )
    return passed + failed + skipped + optional + non_spec, passed, failed, skipped, optional, non_spec


def render() -> str:
    rows, conf_total, conf_ignored = [], 0, 0
    other_suites, other_total = 0, 0
    for path in sorted(TESTS.glob("*.rs")):
        stem = path.stem
        n, ign = count(path)
        if stem in SUITES:
            std, basis = SUITES[stem]
            note = ""
            if stem in CORPUS_RUNNERS:
                if stem in PUBLISH_SCORE:
                    # Parsed on every run, so a stale baseline fails --check.
                    cases, passed, failed, skipped, optional, non_spec = corpus(stem)
                    plural = "" if failed == 1 else "s"
                    note = f"{cases} corpus cases: {passed} pass, {failed} known failure{plural}"
                    if optional:
                        note += f", {optional} optional feature unsupported (reported as the failure the spec requires)"
                    if non_spec:
                        note += (
                            f", {non_spec} expecting behaviour outside the spec "
                            "(reported as the failure the spec requires)"
                        )
                    note += f", {skipped} runner-side skips (floor ≥{CORPUS_RUNNERS[stem]} asserted)"
                elif stem in UNSCORED_NOTES:
                    note = UNSCORED_NOTES[stem]
                else:
                    raise SystemExit(f"{stem}.rs: unscored corpus runner without an UNSCORED_NOTES entry")
            elif stem in FETCHED_RUNNERS:
                note = FETCHED_RUNNERS[stem]
            elif ign:
                note = f"{ign} ignored"
            rows.append((std, f"`tests/{stem}.rs`", basis, n, note))
            conf_total += n
            conf_ignored += ign
        elif stem != "common":
            other_suites += 1
            other_total += n
    lines = [
        "| Standard | Suite | Basis | Tests | Notes |",
        "|---|---|---|---:|---|",
    ]
    for std, suite, basis, n, note in rows:
        lines.append(f"| {std} | {suite} | {basis} | {n} | {note} |")
    lines.append("")
    lines.append(
        f"{conf_total} conformance tests across {len(rows)} suites"
        + (f" ({conf_ignored} ignored)" if conf_ignored else "")
        + f"; a further {other_total} tests in {other_suites} integration, security and "
        "regression suites under `tests/`, plus the crate's unit tests. Only the "
        f"{len([r for r in rows if 'vendored' in r[2]])} **vendored** rows run a published "
        "corpus; every other suite is hand-written and derived from the specification text. "
        "A vendored row gives results only where its corpus licence allows performance claims; "
        "those are development and regression results on the vendored sections "
        "(`docs/conformance/`, `docs/shex.md`, the RML corpora), not W3C, TopQuadrant, OGC or other conformance claims. The W3C "
        "SPARQL 1.1 sections (query, update and federation), the SPARQL 1.2 suite, the RDF 1.2 "
        "syntax suites, the JSON-LD API sections and the OWL 2 DL test cases are partial runs of "
        "W3C test suites, so they carry no results and are used for development and bug tracking "
        "only; the same holds for the W3C R2RML test cases, which CI fetches at a pinned commit "
        "rather than vendoring. The buildingSMART IDS corpus (CC BY-ND 4.0) is downloaded at a "
        "pinned commit too; its results are development results, not a buildingSMART "
        "certification."
    )
    lines.append("")
    lines.append("_Generated by `scripts/conformance_table.py` — edit the suites, not the table._")
    return "\n".join(lines)


def splice(text: str, table: str) -> str:
    a, b = text.index(START), text.index(END)
    return text[: a + len(START)] + "\n" + table + "\n" + text[b:]


def main(argv: list[str]) -> int:
    table = render()
    if "--write" in argv:
        for t in TARGETS:
            t.write_text(splice(t.read_text(encoding="utf-8"), table), encoding="utf-8")
        print(f"updated {len(TARGETS)} files")
        return 0
    if "--check" in argv:
        stale = [t for t in TARGETS if splice(t.read_text(encoding="utf-8"), table) != t.read_text(encoding="utf-8")]
        for t in stale:
            print(f"STALE: {t.relative_to(ROOT)} — run scripts/conformance_table.py --write")
        return 1 if stale else 0
    print(table)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
