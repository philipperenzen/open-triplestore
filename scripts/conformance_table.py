#!/usr/bin/env python3
"""Generate the conformance table from the test suites themselves.

The hand-maintained tables in README.md and docs/standards.md drifted from the
code (112 SPARQL tests claimed vs 125 present, 84 GeoSPARQL vs 107, the W3C SHACL
corpus omitted entirely). This script counts the `#[test]` / `#[tokio::test]`
functions in each `tests/*.rs` suite — a count that matches what `cargo test`
runs, checked for every suite — plus the vendored W3C SHACL corpus, and writes
the result between `<!-- conformance-table:start -->` / `:end -->` markers.

    scripts/conformance_table.py            # print the table
    scripts/conformance_table.py --write    # update README.md and docs/standards.md
    scripts/conformance_table.py --check    # exit 1 if either file is stale (CI)

The *basis* column is the honest part: only the SHACL Core corpus and the OGC
GeoSPARQL validator shapes are vendored, manifest-driven test corpora; every
other suite is hand-written and *derived from* the spec text.
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
    "ogc_geosparql_shacl_roundtrip": ("OGC GeoSPARQL 1.1 validator shapes", "**vendored OGC corpus**"),
    "rdfs_conformance": ("RDFS entailment", "spec-derived"),
    "owl2_rl_conformance": ("OWL 2 RL", "spec-derived"),
    "owl2_el_conformance": ("OWL 2 EL", "spec-derived"),
    "owl2_ql_conformance": ("OWL 2 QL", "spec-derived"),
    "owl2_dl_conformance": ("OWL 2 DL extension rules", "spec-derived"),
    "shacl_conformance": ("SHACL Core", "spec-derived"),
    "w3c_shacl_conformance": ("SHACL Core", "**vendored W3C corpus** (manifest-driven)"),
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


def shacl_corpus() -> tuple[int, int, int, int]:
    """(cases, pass, known failures, runner-side skips) from the runner's own
    recorded baseline (`Empirical baseline: N pass / N known-fail / N aux skips`
    in tests/w3c_shacl_conformance.rs) and its KNOWN_FAILURES list. File counts
    are not used: the corpus directory holds shared/aux files beyond the cases."""
    src = (TESTS / "w3c_shacl_conformance.rs").read_text(encoding="utf-8")
    m = re.search(r"baseline: (\d+) pass / (\d+) known-fail / (\d+) aux skips", src)
    if not m:
        raise SystemExit("w3c_shacl_conformance.rs: baseline comment not found")
    passed, failed, skipped = (int(x) for x in m.groups())
    block = src.split("const KNOWN_FAILURES", 1)[1].split("];", 1)[0]
    known = len(re.findall(r'^\s*\("', block, re.M))
    if known != failed:
        raise SystemExit(f"KNOWN_FAILURES has {known} entries but the baseline says {failed}")
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
            if stem == "w3c_shacl_conformance":
                cases, passed, failed, skipped = shacl_corpus()
                note = f"{cases} corpus cases: {passed} pass, {failed} known failure, {skipped} runner-side skips (floor ≥90 asserted)"
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
        "regression suites under `tests/`, plus the crate's unit tests. Only the two "
        "**vendored** rows run a published corpus; every other suite is hand-written "
        "and derived from the specification text."
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
