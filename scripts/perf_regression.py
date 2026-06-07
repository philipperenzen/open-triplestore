#!/usr/bin/env python3
"""
perf_regression.py — Criterion benchmark regression gate for Open Triplestore.

Reads the machine-readable medians that Criterion writes after `cargo bench`, compares
each benchmark against a committed baseline, and fails when something regresses beyond a
tolerance. Used by the CI `perf` job, the local pre-push hook, and the baseline-refresh
workflow. Pure Python 3 standard library — no third-party imports — so it runs identically
on the maintainer's Windows Git-Bash, GitHub `ubuntu-latest`, and the GitLab `rust:1.88`
image (all ship `python3`).

Criterion output layout (the load-bearing detail)
-------------------------------------------------
For each benchmark `<group>/<id>` Criterion writes:
  target/criterion/<id>/new/estimates.json     ← the MOST RECENT run  (we read this)
  target/criterion/<id>/base/estimates.json    ← previous run, only with --baseline (ignored)
  target/criterion/<id>/change/estimates.json  ← run-over-run delta            (ignored)
We glob `**/new/estimates.json` only, so `base/` and `change/` are never read regardless of
whether `--save-baseline` / `--baseline` was used. The median nanoseconds for a benchmark is
`estimates["median"]["point_estimate"]`. The benchmark id is the path between the criterion
root and the trailing `/new/estimates.json` (e.g. `query/simple_lookup/1000`).

Baseline file (benches/perf_baseline.json)
------------------------------------------
  {
    "schema_version": 1,
    "default_tolerance_ratio": 1.25,      # fail when run/baseline > this (here: +25%)
    "tolerances": { "concurrent/": 1.5, "query/join/10000": 1.4 },
    "generator": { ...provenance metadata, never used in the pass/fail math... },
    "benchmarks": { "query/simple_lookup/1000": 275000.0, ... }   # id -> median nanoseconds
  }

Tolerance precedence for a benchmark id (highest first):
  1. exact key in `tolerances`              (unless --force-tolerance is given)
  2. longest matching prefix key ending in "/" in `tolerances`
  3. --tolerance / OTS_PERF_TOLERANCE override, else `default_tolerance_ratio` (default 1.25)
With --force-tolerance, the CLI/env override beats per-bench and prefix entries too.

Statuses & exit codes
---------------------
  OK         run/baseline <= tolerance
  REGRESSION run/baseline  > tolerance                        -> exit 1
  IMPROVED   run/baseline  < 0.80 (informational; refresh hint)
  WARN       benchmark in run but not baseline (new bench, not yet bootstrapped), or
             benchmark in baseline but not measured this run (PR gate runs a subset)
Exit 0 = no regressions, 1 = at least one regression, 2 = operational error (no Criterion
results found, unreadable/!schema baseline). Finding ZERO estimates is a hard error (exit 2),
mirroring the existing "fail if the filter matched nothing" guard in .github/workflows/ci.yml
so a renamed/broken bench can never make the gate vacuously pass.
"""

import argparse
import glob
import json
import os
import subprocess
import sys
from datetime import datetime, timezone

SCHEMA_VERSION = 1
IMPROVED_RATIO = 0.80


# ─────────────────────────── Criterion parsing ───────────────────────────

def collect_medians(criterion_dir):
    """Return {bench_id: median_ns} parsed from <criterion_dir>/**/new/estimates.json."""
    pattern = os.path.join(criterion_dir, "**", "new", "estimates.json")
    medians = {}
    for path in sorted(glob.glob(pattern, recursive=True)):
        # bench_id is the path between criterion_dir and the trailing /new/estimates.json.
        # The `new` dir is always the immediate parent of estimates.json, so dropping the
        # last two path components yields the id and is robust to group names like "new".
        bench_dir = os.path.dirname(os.path.dirname(path))
        bench_id = os.path.relpath(bench_dir, criterion_dir).replace(os.sep, "/")
        if bench_id in (".", ""):
            continue
        try:
            with open(path, encoding="utf-8") as fh:
                data = json.load(fh)
            medians[bench_id] = float(data["median"]["point_estimate"])
        except (OSError, ValueError, KeyError, TypeError) as exc:
            print(f"warning: skipping unreadable estimates file {path}: {exc}", file=sys.stderr)
    return medians


# ─────────────────────────── Baseline I/O ───────────────────────────

def load_baseline(path):
    try:
        with open(path, encoding="utf-8") as fh:
            baseline = json.load(fh)
    except FileNotFoundError:
        print(f"error: baseline not found: {path}", file=sys.stderr)
        return None
    except (OSError, ValueError) as exc:
        print(f"error: cannot read baseline {path}: {exc}", file=sys.stderr)
        return None
    version = baseline.get("schema_version")
    if version != SCHEMA_VERSION:
        print(
            f"error: baseline schema_version {version!r} != supported {SCHEMA_VERSION} "
            f"({path}). Update this script or the baseline.",
            file=sys.stderr,
        )
        return None
    baseline.setdefault("benchmarks", {})
    baseline.setdefault("tolerances", {})
    baseline.setdefault("default_tolerance_ratio", 1.25)
    return baseline


def resolve_tolerance(bench_id, baseline, override, force):
    """Tolerance ratio for one benchmark id (see module docstring for precedence)."""
    if force and override is not None:
        return override
    tols = baseline.get("tolerances", {})
    if bench_id in tols:
        return float(tols[bench_id])
    best_key = None
    for key in tols:
        if key.endswith("/") and bench_id.startswith(key):
            if best_key is None or len(key) > len(best_key):
                best_key = key
    if best_key is not None:
        return float(tols[best_key])
    if override is not None:
        return override
    return float(baseline.get("default_tolerance_ratio", 1.25))


# ─────────────────────────── Provenance metadata (update) ───────────────────────────

def git_short_commit():
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, check=True,
        )
        return out.stdout.strip() or None
    except (OSError, subprocess.SubprocessError):
        return None


def detect_cpu():
    try:
        with open("/proc/cpuinfo", encoding="utf-8") as fh:
            for line in fh:
                if line.lower().startswith("model name"):
                    return line.split(":", 1)[1].strip()
    except OSError:
        pass
    import platform
    return platform.processor() or platform.machine() or "unknown"


# ─────────────────────────── Reporting ───────────────────────────

def human_ns(ns):
    for unit, scale in (("s", 1e9), ("ms", 1e6), ("µs", 1e3)):
        if ns >= scale:
            return f"{ns / scale:.3g} {unit}"
    return f"{ns:.0f} ns"


def render_markdown(rows, summary):
    lines = [
        "| Benchmark | baseline | this run | Δ% | status |",
        "|---|--:|--:|--:|:--|",
    ]
    for r in rows:
        base = human_ns(r["baseline"]) if r["baseline"] is not None else "—"
        run = human_ns(r["run"]) if r["run"] is not None else "—"
        delta = f"{(r['ratio'] - 1) * 100:+.1f}%" if r["ratio"] is not None else "—"
        lines.append(f"| `{r['id']}` | {base} | {run} | {delta} | {r['status']} |")
    lines.append("")
    lines.append(summary)
    return "\n".join(lines)


# ─────────────────────────── Subcommands ───────────────────────────

def cmd_check(args):
    baseline = load_baseline(args.baseline)
    if baseline is None:
        return 2
    runs = collect_medians(args.criterion_dir)
    if not runs:
        print(
            f"error: no Criterion results under {args.criterion_dir} (no **/new/estimates.json). "
            "Did the benchmark run? Refusing to pass vacuously.",
            file=sys.stderr,
        )
        return 2

    base_benches = baseline["benchmarks"]
    override = args.tolerance
    if override is None and os.environ.get("OTS_PERF_TOLERANCE"):
        try:
            override = float(os.environ["OTS_PERF_TOLERANCE"])
        except ValueError:
            print("warning: ignoring non-numeric OTS_PERF_TOLERANCE", file=sys.stderr)

    rows, regressions, improvements, warnings = [], 0, 0, 0
    for bench_id in sorted(set(runs) | set(base_benches)):
        run_ns = runs.get(bench_id)
        base_ns = base_benches.get(bench_id)
        if run_ns is not None and base_ns is not None and base_ns > 0:
            tol = resolve_tolerance(bench_id, baseline, override, args.force_tolerance)
            ratio = run_ns / base_ns
            if ratio > tol:
                status, regressions = f"REGRESSION (>{tol:.2f}x)", regressions + 1
            elif ratio < IMPROVED_RATIO:
                status, improvements = "IMPROVED", improvements + 1
            else:
                status = "ok"
            rows.append({"id": bench_id, "baseline": base_ns, "run": run_ns, "ratio": ratio, "status": status})
        elif run_ns is not None:
            warnings += 1
            rows.append({"id": bench_id, "baseline": None, "run": run_ns, "ratio": None,
                         "status": "WARN: new (not in baseline)"})
        else:
            warnings += 1
            rows.append({"id": bench_id, "baseline": base_ns, "run": None, "ratio": None,
                         "status": "WARN: not measured this run"})

    rows.sort(key=lambda r: (r["ratio"] is None, -(r["ratio"] or 0)))
    compared = regressions + improvements + sum(
        1 for r in rows if r["ratio"] is not None and "REGRESSION" not in r["status"] and "IMPROVED" not in r["status"]
    )
    summary = (
        f"**{compared}** benchmarks compared · **{regressions}** regressions · "
        f"**{improvements}** improved · **{warnings}** warnings"
    )
    report = render_markdown(rows, summary)
    print(report)

    if args.github_summary and os.environ.get("GITHUB_STEP_SUMMARY"):
        try:
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as fh:
                fh.write("## Performance regression gate\n\n" + report + "\n")
        except OSError as exc:
            print(f"warning: could not write GITHUB_STEP_SUMMARY: {exc}", file=sys.stderr)

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as fh:
            json.dump({"summary": {"compared": compared, "regressions": regressions,
                                   "improved": improvements, "warnings": warnings},
                       "rows": rows}, fh, indent=2)

    if warnings and args.fail_on_missing_baseline and any(r["baseline"] is None for r in rows):
        print("error: new benchmarks missing from baseline and --fail-on-missing-baseline set",
              file=sys.stderr)
        return 1
    return 1 if regressions else 0


def cmd_update(args):
    runs = collect_medians(args.criterion_dir)
    if not runs:
        print(f"error: no Criterion results under {args.criterion_dir}; nothing to write.",
              file=sys.stderr)
        return 2

    existing = {}
    if args.keep_tolerances and os.path.exists(args.out):
        loaded = load_baseline(args.out)
        if loaded is not None:
            existing = loaded

    baseline = {
        "schema_version": SCHEMA_VERSION,
        "default_tolerance_ratio": existing.get("default_tolerance_ratio", 1.25),
        "tolerances": existing.get("tolerances", {"concurrent/": 1.5}),
        "generator": {
            "commit": git_short_commit() or "unknown",
            "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "runner": args.runner or "local",
            "cpu": args.cpu or detect_cpu(),
            "command": "cargo bench --bench performance --features full",
            "criterion_metric": "median.point_estimate",
            "unit": "ns",
            "note": ("Authoritative perf baseline. Refresh ONLY via the perf-baseline workflow "
                     "(version tag or manual dispatch) — never edit by hand or from a PR run."),
        },
        "benchmarks": {k: round(v, 1) for k, v in sorted(runs.items())},
    }
    with open(args.out, "w", encoding="utf-8") as fh:
        json.dump(baseline, fh, indent=2)
        fh.write("\n")
    print(f"wrote {len(runs)} benchmarks to {args.out} "
          f"(commit {baseline['generator']['commit']}, runner {baseline['generator']['runner']})")
    return 0


# ─────────────────────────── CLI ───────────────────────────

def main(argv=None):
    # The report contains a few non-ASCII glyphs (Δ, µs). Force UTF-8 on the streams so a
    # cp1252 Windows console or a LANG=C CI shell can't crash the gate with UnicodeEncodeError.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8")
        except (AttributeError, ValueError):
            pass

    parser = argparse.ArgumentParser(description=__doc__.splitlines()[1],
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command")

    chk = sub.add_parser("check", help="compare a benchmark run against the baseline (default)")
    chk.add_argument("--criterion-dir", default="target/criterion")
    chk.add_argument("--baseline", default="benches/perf_baseline.json")
    chk.add_argument("--tolerance", type=float, default=None,
                     help="override default_tolerance_ratio (env: OTS_PERF_TOLERANCE)")
    chk.add_argument("--force-tolerance", action="store_true",
                     help="make --tolerance override per-bench/prefix entries too")
    chk.add_argument("--github-summary", action="store_true",
                     help="also append the table to $GITHUB_STEP_SUMMARY")
    chk.add_argument("--fail-on-missing-baseline", action="store_true",
                     help="treat 'benchmark not in baseline' as an error")
    chk.add_argument("--json-out", default=None, help="also dump a machine-readable result")
    chk.set_defaults(func=cmd_check)

    upd = sub.add_parser("update", help="(re)generate the baseline from a fresh run")
    upd.add_argument("--criterion-dir", default="target/criterion")
    upd.add_argument("--out", default="benches/perf_baseline.json")
    upd.add_argument("--runner", default=None, help="provenance: where this ran")
    upd.add_argument("--cpu", default=None, help="provenance: CPU (auto-detected on Linux)")
    upd.add_argument("--keep-tolerances", action="store_true",
                     help="preserve default_tolerance_ratio + tolerances from the existing file")
    upd.set_defaults(func=cmd_update)

    args = parser.parse_args(argv)
    if not getattr(args, "command", None):
        # Default to `check` with defaults when invoked bare.
        args = parser.parse_args(["check"] + (argv or []))
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
