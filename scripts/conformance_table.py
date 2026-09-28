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

The *basis* column is the honest part: only the W3C SPARQL 1.1 query/update
sections, the W3C SHACL core/sparql sections and the OGC GeoSPARQL validator
shapes are vendored test corpora; every other suite is hand-written and
*derived from* the spec text.

Which corpus results are published is a licence question, not a style one:

- The SPARQL 1.1 sections come from a W3C test suite, which W3C licenses under
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
- The SHACL sections are under the W3C Software and Document License, which
  sets no such condition, so that row keeps its counts (`PUBLISH_SCORE`).
- The OGC validator shapes are under the Apache License 2.0; only the OGC
  authorises compliance marks for its standards, so no row claims compliance.
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
    "sparql12_conformance": ("SPARQL 1.2 / RDF-star", "spec-derived"),
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
    "owl2_dl_conformance": ("OWL 2 DL extension rules", "spec-derived"),
    "shacl_conformance": ("SHACL Core", "spec-derived"),
    "w3c_shacl_conformance": ("SHACL Core", "**vendored W3C corpus** (core + sparql sections, manifest-driven)"),
    "w3c_sparql11_manifests": ("SPARQL 1.1 Query/Update", "**vendored W3C test-suite subset** (query + update sections of w3c/rdf-tests, unmodified; manifest-driven)"),
    "shacl_rules_conformance": ("SHACL-AF rules", "spec-derived"),
    "shaclc_conformance": ("SHACL Compact Syntax", "spec-derived"),
    "shex_conformance": ("ShEx", "spec-derived"),
    "swrl_conformance": ("SWRL", "spec-derived"),
    "ldp_conformance": ("LDP 1.0 (store level)", "spec-derived"),
    "ldp_http_conformance": ("LDP 1.0 (HTTP)", "spec-derived"),
    "dcat_conformance": ("DCAT 2 / VoID", "spec-derived"),
    "rml_conformance": ("RML / R2RML", "spec-derived"),
    "standards_conformance": ("Cross-standard HTTP smoke", "spec-derived"),
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
    "w3c_shacl_conformance": 90,
    "w3c_sparql11_manifests": 450,
}

# Runners whose score may be published (see the module docstring). A runner
# missing here is still checked, but its row carries no numbers: the W3C SPARQL
# 1.1 sections are a subset of a W3C test suite, on which W3C's test-suite
# policy allows no public performance claims.
PUBLISH_SCORE = {"w3c_shacl_conformance"}

# The note for a corpus runner whose score is not published.
UNSCORED_NOTE = (
    "runs in CI as a development and regression ratchet; no score is published "
    "(W3C test-suite policy); known gaps in `docs/conformance/sparql11.md`"
)


def corpus(stem: str) -> tuple[int, int, int, int]:
    """(cases, pass, known failures, runner-side skips) from the runner's own
    recorded baseline (`Empirical baseline: N pass / N known-fail / N aux skips`
    in tests/<stem>.rs) and its KNOWN_FAILURES list. File counts are not used:
    the corpus directories hold shared/aux files beyond the cases."""
    src = (TESTS / f"{stem}.rs").read_text(encoding="utf-8")
    m = re.search(r"baseline: (\d+) pass / (\d+) known-fail / (\d+) aux skips", src)
    if not m:
        raise SystemExit(f"{stem}.rs: baseline comment not found")
    passed, failed, skipped = (int(x) for x in m.groups())
    block = src.split("const KNOWN_FAILURES", 1)[1].split("];", 1)[0]
    known = len(re.findall(r'^\s*\("', block, re.M))
    if known != failed:
        raise SystemExit(f"{stem}.rs: KNOWN_FAILURES has {known} entries but the baseline says {failed}")
    return passed + failed + skipped, passed, failed, skipped


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
                    cases, passed, failed, skipped = corpus(stem)
                    plural = "" if failed == 1 else "s"
                    note = f"{cases} corpus cases: {passed} pass, {failed} known failure{plural}, {skipped} runner-side skips (floor ≥{CORPUS_RUNNERS[stem]} asserted)"
                else:
                    note = UNSCORED_NOTE
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
        "The SHACL and GeoSPARQL corpus results are development and regression results on the "
        "vendored sections (`docs/conformance/`), not W3C or OGC conformance claims. The SPARQL "
        "1.1 sections are a subset of a W3C test suite, so under W3C's test-suite licence policy "
        "they are used for development and bug tracking only, and no score is published for them."
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
