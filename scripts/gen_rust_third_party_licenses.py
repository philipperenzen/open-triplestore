#!/usr/bin/env python3
"""Write the licence notices of the Rust crates compiled into the server binary.

The open-triplestore binary statically links a few hundred crates from
crates.io, most under MIT, BSD, ISC or Apache-2.0. Those licences let the
binary be redistributed on condition that it carries the crates' copyright
notices and licence texts (and, for Apache-2.0 §4(d), their NOTICE files).
This script collects them into one text file that ships next to NOTICE: the
Docker build runs it in the builder stage right after `cargo build` and copies
the result to /app/THIRD-PARTY-LICENSES-server.txt.

Which crates: `cargo tree -e normal,no-proc-macro` for the root package, with
the features and target of the build, so it lists what is linked into the
binary: dev-dependencies, build-dependencies (build scripts, cc, bindgen) and
procedural macros run only at compile time and are left out. The workspace's
own crates (open-triplestore, opengraph, the plugins) are Open Triplestore
itself and are listed separately, without texts.

What goes in, per crate: the licence expression from its Cargo.toml and,
verbatim, every LICENSE / LICENCE / COPYING / NOTICE / COPYRIGHT / UNLICENSE
file in the crate's source directory (plus its `license-file`), including the
ones in subdirectories that hold bundled C/C++ sources (for example RocksDB,
LZ4 and zstd inside the -sys crates). Directories of tests, examples, docs,
benchmarks and command-line programs are skipped: their code is not linked.
Identical texts are printed once, followed by the crates they belong to.

Only the standard library and cargo are needed (the rust:*-bookworm builder
image has Python 3). The crates' sources must be available locally, which they
are after a build; pass --offline to forbid network access.

Usage:
    python3 scripts/gen_rust_third_party_licenses.py \\
        --features full --output THIRD-PARTY-LICENSES-server.txt
    # the target defaults to the host; to see what a Linux image links:
    python3 scripts/gen_rust_third_party_licenses.py --features full \\
        --target x86_64-unknown-linux-gnu --output /tmp/licences.txt
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import textwrap
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Licence-bearing file names (case-insensitive): LICENSE, LICENSE-MIT,
# LICENSE.Apache, LICENCE.txt, COPYING, COPYING.LESSER, NOTICE, COPYRIGHT,
# UNLICENSE, ...
LICENSE_FILE_RE = re.compile(
    r"^(licen[cs]e|copying|notice|copyright|unlicen[cs]e)([-._ ].*)?$", re.IGNORECASE
)
# Subdirectories whose contents are not compiled into a library.
SKIP_DIRS = {
    "tests", "test", "testdata", "test-data", "testing", "examples", "example",
    "benches", "bench", "benchmarks", "docs", "doc", "fuzz", "programs",
    "test_data", "contrib", "ci", "scripts", "tools", "target", "node_modules",
}
# How to recognise a complete copy of a licence among the texts collected, so
# a crate that ships no licence file can be pointed at one. Every phrase must
# occur in the text; `absent` phrases must not.
LICENCE_SIGNATURES: dict[str, dict[str, list[str]]] = {
    "Apache-2.0": {"all": ["Apache License", "Version 2.0, January 2004",
                           "END OF TERMS AND CONDITIONS"], "absent": []},
    "MIT": {"all": ["Permission is hereby granted, free of charge",
                    'THE SOFTWARE IS PROVIDED "AS IS"'], "absent": []},
    "MPL-2.0": {"all": ["Mozilla Public License Version 2.0", "Exhibit A"], "absent": []},
    "BSD-3-Clause": {"all": ["Redistributions of source code must retain",
                             "Redistributions in binary form must reproduce",
                             "endorse or promote products"], "absent": []},
    "BSD-2-Clause": {"all": ["Redistributions of source code must retain",
                             "Redistributions in binary form must reproduce"],
                     "absent": ["endorse or promote products"]},
    "ISC": {"all": ["Permission to use, copy, modify, and/or distribute this software for any purpose"],
            "absent": []},
    "Unlicense": {"all": ["This is free and unencumbered software released into the public domain"],
                  "absent": []},
    "Zlib": {"all": ["This software is provided 'as-is', without any express or implied",
                     "Altered source versions must be plainly marked"], "absent": []},
    "BSL-1.0": {"all": ["Boost Software License - Version 1.0"], "absent": []},
}
MAX_DEPTH = 4  # levels below the crate root to look for bundled sources' licences
MAX_TEXT_BYTES = 512 * 1024  # skip anything larger: not a licence file

RULE = "=" * 78
THIN = "-" * 78


def run(cmd: list[str]) -> str:
    try:
        return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout
    except subprocess.CalledProcessError as e:
        sys.stderr.write(e.stderr)
        raise SystemExit(f"gen_rust_third_party_licenses: `{' '.join(cmd)}` failed")


def host_target() -> str:
    for line in run(["rustc", "-vV"]).splitlines():
        if line.startswith("host: "):
            return line.split(": ", 1)[1].strip()
    raise SystemExit("gen_rust_third_party_licenses: cannot tell the host target from `rustc -vV`")


def cargo_flags(args: argparse.Namespace) -> list[str]:
    flags = ["--manifest-path", str(args.manifest_path), "--locked"]
    if args.offline:
        flags.append("--offline")
    if args.features:
        flags += ["--features", args.features]
    return flags


def linked_crates(args: argparse.Namespace) -> set[tuple[str, str, str]]:
    """(name, version, source-or-path) of every crate in the normal,
    non-proc-macro dependency tree of the root package, root included."""
    # `--color never`: with CARGO_TERM_COLOR=always (as CI sets it) cargo
    # colours the `(*)` dedup marker even into a pipe.
    out = run(
        ["cargo", "tree", *cargo_flags(args), "-p", args.package, "--target", args.target,
         "-e", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}",
         "--color", "never"]
    )
    crates = set()
    for line in out.splitlines():
        line = line.strip().removesuffix("(*)").strip()
        if not line:
            continue
        m = re.match(r"^(\S+) v(\S+)(?: \((.*)\))?$", line)
        if not m:
            raise SystemExit(f"gen_rust_third_party_licenses: unexpected `cargo tree` line: {line!r}")
        crates.add((m.group(1), m.group(2), m.group(3) or ""))
    return crates


def metadata(args: argparse.Namespace) -> dict:
    return json.loads(
        run(["cargo", "metadata", "--format-version", "1", *cargo_flags(args),
             "--filter-platform", args.target])
    )


def licence_files(crate_dir: Path, license_file: str | None) -> list[Path]:
    """Licence-bearing files of a crate, root first, then subdirectories."""
    found: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(crate_dir):
        rel = Path(dirpath).relative_to(crate_dir)
        depth = len(rel.parts)
        dirnames[:] = sorted(
            d for d in dirnames
            if depth < MAX_DEPTH and d.lower() not in SKIP_DIRS and not d.startswith(".")
        )
        for f in sorted(filenames):
            if LICENSE_FILE_RE.match(f):
                p = Path(dirpath) / f
                if p.is_file() and p.stat().st_size <= MAX_TEXT_BYTES:
                    found.append(p)
    if license_file:
        p = (crate_dir / license_file).resolve()
        if p.is_file() and p not in [q.resolve() for q in found]:
            found.insert(0, p)
    found.sort(key=lambda p: (len(p.relative_to(crate_dir).parts) if p.is_relative_to(crate_dir) else 0, str(p)))
    return found


def read_text(p: Path) -> str:
    data = p.read_bytes()
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("latin-1")


def dedup_key(text: str) -> str:
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    return "\n".join(line.rstrip() for line in text.split("\n")).strip()


def licence_ids(expr: str | None) -> list[str]:
    """The licence identifiers in an SPDX expression (or a legacy "MIT/Apache-2.0")."""
    if not expr:
        return []
    ids = []
    for tok in re.findall(r"[A-Za-z0-9.+-]+", expr):
        if tok.upper() in {"OR", "AND", "WITH"} or tok.endswith("-exception"):
            continue
        if tok not in ids:
            ids.append(tok)
    return ids


def reference_texts(order: list[dict]) -> dict[str, int]:
    """For each licence in LICENCE_SIGNATURES, the shortest collected text that
    contains a complete copy of it: a plain licence file rather than a
    compilation of several."""
    best: dict[str, tuple[int, int]] = {}
    for slot in order:
        text = " ".join(slot["text"].split())
        for spdx, sig in LICENCE_SIGNATURES.items():
            if all(" ".join(ph.split()) in text for ph in sig["all"]) and not any(
                    " ".join(ph.split()) in text for ph in sig["absent"]):
                if spdx not in best or len(text) < best[spdx][0]:
                    best[spdx] = (len(text), slot["n"])
    return {spdx: n for spdx, (_, n) in best.items()}


def para(text: str, indent: str = "") -> str:
    return textwrap.fill(" ".join(text.split()), width=78, initial_indent=indent,
                         subsequent_indent=indent, break_on_hyphens=False, break_long_words=False)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--manifest-path", type=Path, default=REPO / "Cargo.toml")
    ap.add_argument("--package", default="open-triplestore")
    ap.add_argument("--features", default="full", help="the build's --features (default: full)")
    ap.add_argument("--target", default=None, help="target triple (default: the host)")
    ap.add_argument("--offline", action="store_true", help="pass --offline to cargo")
    ap.add_argument("--output", type=Path, required=True)
    args = ap.parse_args()
    args.target = args.target or host_target()

    meta = metadata(args)
    workspace = set(meta["workspace_members"])
    by_name_version: dict[tuple[str, str], list[dict]] = {}
    for pkg in meta["packages"]:
        by_name_version.setdefault((pkg["name"], pkg["version"]), []).append(pkg)

    first_party: list[dict] = []
    crates: list[dict] = []
    for name, version, where in sorted(linked_crates(args)):
        cands = by_name_version.get((name, version), [])
        if len(cands) > 1 and where:
            cands = [c for c in cands if where in (c.get("source") or "")
                     or where == str(Path(c["manifest_path"]).parent)] or cands
        if not cands:
            raise SystemExit(f"gen_rust_third_party_licenses: {name} {version} is not in `cargo metadata`")
        pkg = cands[0]
        (first_party if pkg["id"] in workspace else crates).append(pkg)

    texts: dict[str, dict] = {}
    order: list[dict] = []
    refs: dict[str, list[int]] = {}
    missing: list[str] = []
    for pkg in crates:
        key = f"{pkg['name']} {pkg['version']}"
        crate_dir = Path(pkg["manifest_path"]).parent
        nums: list[int] = []
        for f in licence_files(crate_dir, pkg.get("license_file")):
            text = read_text(f)
            if not text.strip():
                continue
            k = dedup_key(text)
            slot = texts.get(k)
            if slot is None:
                slot = {"text": text, "users": [], "n": len(order) + 1}
                texts[k] = slot
                order.append(slot)
            rel = f.relative_to(crate_dir) if f.is_relative_to(crate_dir) else f.name
            slot["users"].append(f"{key} ({rel.as_posix() if isinstance(rel, Path) else rel})")
            if slot["n"] not in nums:
                nums.append(slot["n"])
        refs[key] = nums
        if not nums:
            missing.append(key)

    out: list[str] = [RULE, "Third-party software notices and licences: Open Triplestore server", RULE, ""]
    out += [para(
        "Generated by scripts/gen_rust_third_party_licenses.py from Cargo.lock and the crates' "
        f"sources, for the features \"{args.features}\" and the target {args.target}. The Docker "
        "build regenerates it for each image. Do not edit it by hand."), ""]
    out += [para(
        "The open-triplestore server binary is compiled from Open Triplestore's own crates and "
        "the third-party Rust crates listed in section 1, which are linked into it. Each crate "
        "is distributed under its own licence, shown next to it as its Cargo.toml declares it "
        "(an SPDX expression; for a choice such as \"MIT OR Apache-2.0\" either licence may be "
        "used). The AGPL-3.0 and the Commons Clause that cover Open Triplestore do not apply to "
        "these crates. Crates that run only while compiling (build scripts, procedural macros) "
        "and development-only crates are not part of the binary and are not listed."), ""]
    out += [para(
        "Section 2 reproduces, verbatim, every licence, copying and notice file those crates "
        "ship, including those of the C and C++ sources some of them bundle and compile in "
        "(a crate may bundle more sources than this build compiles). "
        "Each distinct text appears once, followed by the crates and files it comes from; the "
        "numbers in [brackets] in section 1 refer to these texts."), ""]
    out += [para(
        "Not covered here: shared system libraries the binary loads at run time (GEOS, OpenSSL, "
        "libxml2, xmlsec, the C library) come from the operating system's packages, whose "
        "copyright files are in /usr/share/doc/ of the Docker image; the web UI's notices are "
        "in frontend/dist/THIRD-PARTY-LICENSES.txt; everything else Open Triplestore bundles "
        "is in NOTICE and LICENSES/."), ""]
    out += [para("Open Triplestore's own crates compiled in: " + ", ".join(
        f"{p['name']} {p['version']}" + (f" ({p['license']})" if p.get("license") else "")
        for p in first_party) + ". Unless marked otherwise they are covered by LICENSE."), ""]

    ref_texts = reference_texts(order)
    out += [RULE, f"1. Crates ({len(crates)})", RULE, ""]
    for pkg in crates:
        key = f"{pkg['name']} {pkg['version']}"
        out.append(key)
        lic = pkg.get("license") or (f"see {pkg['license_file']}" if pkg.get("license_file") else "(none declared)")
        out.append(f"  Licence: {lic}")
        if pkg.get("repository"):
            out.append(f"  Source:  {pkg['repository']}")
        nums = refs[key]
        if nums:
            out.append(para("Texts: " + " ".join(f"[{n}]" for n in nums), indent="  "))
        else:
            out.append("  Texts: no licence or notice file in the crate")
            known = [(i, ref_texts[i]) for i in licence_ids(pkg.get("license")) if i in ref_texts]
            if known:
                out.append(para("Licence text: " + ", ".join(f"{i} [{n}]" for i, n in known)
                                + " (as shipped by another crate)", indent="  "))
            if pkg.get("authors"):
                out.append(para("Authors (Cargo.toml): " + ", ".join(pkg["authors"]), indent="  "))
        out.append("")
    if missing:
        out += [para(
            "Crates marked \"no licence or notice file in the crate\" publish none in their "
            "crates.io package; their terms are the licence their Cargo.toml declares, shown "
            "above, whose full text section 2 reproduces from another crate where marked "
            "\"Licence text\" (read the copyright line in it as that crate's, not theirs). "
            "Their authors, as their Cargo.toml names them, are listed with them."), ""]

    out += [RULE, "2. Licence and notice texts", RULE]
    for slot in order:
        out += ["", THIN, f"[{slot['n']}]"]
        out += [para(u, indent="  ") for u in slot["users"]]
        out += [THIN, ""]
        out.append(slot["text"].replace("\r\n", "\n").replace("\r", "\n").rstrip())
    out.append("")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(out), encoding="utf-8")
    print(f"gen_rust_third_party_licenses: {args.output}: {len(crates)} crates, "
          f"{len(order)} distinct texts, {len(first_party)} first-party crates")
    if missing:
        print(f"gen_rust_third_party_licenses: {len(missing)} crates ship no licence file: "
              + ", ".join(missing), file=sys.stderr)


if __name__ == "__main__":
    main()
