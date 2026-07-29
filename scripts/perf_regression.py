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
    "default_tolerance_ratio": 1.10,      # fail when run/baseline > this (here: +10%)
    "tolerances": { "concurrent/": 1.5, "query/join/10000": 1.4 },
    "generator": { ...provenance metadata, never used in the pass/fail math... },
    "benchmarks": { "query/simple_lookup/1000": 275000.0, ... }   # id -> median nanoseconds
  }

Tolerance precedence for a benchmark id (highest first):
  1. exact key in `tolerances`              (unless --force-tolerance is given)
  2. longest matching prefix key ending in "/" in `tolerances`
  3. `small_benchmark_tolerance` when the reference side is under `small_benchmark_ns`
  4. --tolerance / OTS_PERF_TOLERANCE override, else `default_tolerance_ratio` (default 1.10)
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
# Fallback when a baseline omits `default_tolerance_ratio`. The committed baseline
# sets it explicitly; this only covers a hand-rolled or bootstrap file.
DEFAULT_TOLERANCE = 1.10
# Below this many nanoseconds a percentage bar stops carrying information — see
# `resolve_tolerance`. Both are overridable per baseline file.
SMALL_BENCHMARK_NS = 1000.0
SMALL_BENCHMARK_TOLERANCE = 1.35


# ─────────────────────────── Criterion parsing ───────────────────────────

def collect_medians(criterion_dirs):
    """Return {bench_id: median_ns} parsed from <dir>/**/new/estimates.json.

    Takes a *list* of Criterion output dirs — one per repeat of the same benchmark
    run — and keeps the FASTEST median per benchmark.

    Criterion's median is already robust to noise *within* one process: it discards
    nothing, but the outliers it reports are spread across many samples. What it
    cannot see is interference that lasts the whole process — a noisy neighbour on
    a shared CI runner slows every sample equally, so the median moves with it. That
    is the dominant error term here: two runs of the *same commit* against the *same*
    baseline moved a benchmark's ratio by up to 26 percentage points, and 11 of 68
    benchmarks moved by more than 10.

    Interference can only ever make a benchmark look slower, never faster, so the
    minimum across repeats is the estimator that discards it: whichever run happened
    to get the quietest machine is the one closest to the true cost. Pass a single
    dir for the old single-run behaviour.
    """
    medians = {}
    for criterion_dir in criterion_dirs:
        pattern = os.path.join(criterion_dir, "**", "new", "estimates.json")
        for path in sorted(glob.glob(pattern, recursive=True)):
            # bench_id is the path between criterion_dir and the trailing
            # /new/estimates.json. The `new` dir is always the immediate parent of
            # estimates.json, so dropping the last two path components yields the id
            # and is robust to group names like "new".
            bench_dir = os.path.dirname(os.path.dirname(path))
            bench_id = os.path.relpath(bench_dir, criterion_dir).replace(os.sep, "/")
            if bench_id in (".", ""):
                continue
            try:
                with open(path, encoding="utf-8") as fh:
                    data = json.load(fh)
                median = float(data["median"]["point_estimate"])
            except (OSError, ValueError, KeyError, TypeError) as exc:
                print(f"warning: skipping unreadable estimates file {path}: {exc}", file=sys.stderr)
                continue
            previous = medians.get(bench_id)
            medians[bench_id] = median if previous is None else min(previous, median)
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
    baseline.setdefault("default_tolerance_ratio", DEFAULT_TOLERANCE)
    return baseline


def resolve_tolerance(bench_id, baseline, override, force, reference_ns=None):
    """Tolerance ratio for one benchmark id (see module docstring for precedence).

    `reference_ns` is what the benchmark took on the side being compared against.
    Below `small_benchmark_ns` the default does not apply, because a percentage bar
    stops meaning anything down there: `geosparql_sf_contains/50` runs in 79 ns, so
    a single scheduling hiccup worth 15 ns reads as +19%. Naming such benchmarks
    one at a time in `tolerances` only ever catches the one that happened to trip
    last — the floor covers the class.
    """
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
    small_ns = baseline.get("small_benchmark_ns")
    if small_ns and reference_ns is not None and reference_ns < float(small_ns):
        return float(baseline.get("small_benchmark_tolerance", SMALL_BENCHMARK_TOLERANCE))
    if override is not None:
        return override
    return float(baseline.get("default_tolerance_ratio", DEFAULT_TOLERANCE))


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
    runs = collect_medians(args.criterion_dirs or ["target/criterion"])
    if not runs:
        print(
            f"error: no Criterion results under {args.criterion_dirs or ['target/criterion']} "
            "(no **/new/estimates.json). "
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
            tol = resolve_tolerance(bench_id, baseline, override, args.force_tolerance,
                                    reference_ns=base_ns)
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


def cmd_compare(args):
    """Gate a change against its own merge base, both measured in the same job.

    `check` compares against the committed baseline, which was recorded on some
    *other* runner instance days or weeks earlier — so runner speed lands directly
    in the ratio. Measured on this repo, five gate runs of unchanged code spread by
    a median of 1.133 and a max of 1.441, and 50 of 68 benchmarks could exceed a
    +10% bar on noise alone. No tolerance table fixes that; the comparison itself
    has to change.

    Benching both revisions back-to-back on the same machine cancels the runner:
    a slow VM makes *both* sides slow, and the ratio holds. What is left is the
    drift between the two halves of one job, which the repeated passes (`--before`
    / `--after` given once per pass, fastest median wins) absorb.

    Tolerances still come from the baseline file, so the `tolerances` map and
    `default_tolerance_ratio` keep working and stay in one place.
    """
    baseline = load_baseline(args.baseline)
    if baseline is None:
        return 2
    before = collect_medians(args.before)
    after = collect_medians(args.after)
    for label, got, dirs in (("before", before, args.before), ("after", after, args.after)):
        if not got:
            print(
                f"error: no Criterion results for the '{label}' side under {dirs} "
                "(no **/new/estimates.json). Refusing to pass vacuously.",
                file=sys.stderr,
            )
            return 2

    override = args.tolerance
    if override is None and os.environ.get("OTS_PERF_TOLERANCE"):
        try:
            override = float(os.environ["OTS_PERF_TOLERANCE"])
        except ValueError:
            print("warning: ignoring non-numeric OTS_PERF_TOLERANCE", file=sys.stderr)

    rows, regressions, improvements, warnings = [], 0, 0, 0
    for bench_id in sorted(set(before) | set(after)):
        base_ns, run_ns = before.get(bench_id), after.get(bench_id)
        if run_ns is not None and base_ns is not None and base_ns > 0:
            tol = resolve_tolerance(bench_id, baseline, override, args.force_tolerance,
                                    reference_ns=base_ns)
            ratio = run_ns / base_ns
            if ratio > tol:
                status, regressions = f"REGRESSION (>{tol:.2f}x)", regressions + 1
            elif ratio < IMPROVED_RATIO:
                status, improvements = "IMPROVED", improvements + 1
            else:
                status = "ok"
            rows.append({"id": bench_id, "baseline": base_ns, "run": run_ns,
                         "ratio": ratio, "status": status})
        elif run_ns is not None:
            warnings += 1
            rows.append({"id": bench_id, "baseline": None, "run": run_ns, "ratio": None,
                         "status": "WARN: only on this change (new benchmark)"})
        else:
            warnings += 1
            rows.append({"id": bench_id, "baseline": base_ns, "run": None, "ratio": None,
                         "status": "WARN: only on the merge base (benchmark removed)"})

    rows.sort(key=lambda r: (r["ratio"] is None, -(r["ratio"] or 0)))
    compared = sum(1 for r in rows if r["ratio"] is not None)
    summary = (
        f"**{compared}** benchmarks compared against the merge base · "
        f"**{regressions}** regressions · **{improvements}** improved · "
        f"**{warnings}** warnings"
    )
    report = render_markdown(rows, summary).replace(
        "| Benchmark | baseline | this run |", "| Benchmark | merge base | this change |"
    )
    print(report)

    if args.github_summary and os.environ.get("GITHUB_STEP_SUMMARY"):
        try:
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as fh:
                fh.write("## Performance vs merge base\n\n" + report + "\n")
        except OSError as exc:
            print(f"warning: could not write GITHUB_STEP_SUMMARY: {exc}", file=sys.stderr)

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as fh:
            json.dump({"summary": {"compared": compared, "regressions": regressions,
                                   "improved": improvements, "warnings": warnings},
                       "rows": rows}, fh, indent=2)

    return 1 if regressions else 0


def cmd_update(args):
    runs = collect_medians(args.criterion_dirs or ["target/criterion"])
    if not runs:
        print(f"error: no Criterion results under {args.criterion_dirs or ['target/criterion']}; "
              "nothing to write.",
              file=sys.stderr)
        return 2

    existing = {}
    if args.keep_tolerances and os.path.exists(args.out):
        loaded = load_baseline(args.out)
        if loaded is not None:
            existing = loaded

    baseline = {
        "schema_version": SCHEMA_VERSION,
        "default_tolerance_ratio": existing.get("default_tolerance_ratio", DEFAULT_TOLERANCE),
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
    chk.add_argument("--criterion-dir", action="append", dest="criterion_dirs",
                     metavar="DIR",
                     help="Criterion output dir; repeat once per benchmark repeat "
                          "(the fastest median per benchmark wins). "
                          "Default: target/criterion")
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

    cmp_ = sub.add_parser("compare",
                          help="gate a change against its merge base, both measured in one job")
    cmp_.add_argument("--before", action="append", default=[], metavar="DIR",
                      help="Criterion output dir for the merge base; repeat once per pass")
    cmp_.add_argument("--after", action="append", default=[], metavar="DIR",
                      help="Criterion output dir for the change; repeat once per pass")
    cmp_.add_argument("--baseline", default="benches/perf_baseline.json",
                      help="read tolerances from here (its measured numbers are not used)")
    cmp_.add_argument("--tolerance", type=float, default=None,
                      help="override default_tolerance_ratio (env: OTS_PERF_TOLERANCE)")
    cmp_.add_argument("--force-tolerance", action="store_true",
                      help="make --tolerance override per-bench/prefix entries too")
    cmp_.add_argument("--github-summary", action="store_true",
                      help="also append the table to $GITHUB_STEP_SUMMARY")
    cmp_.add_argument("--json-out", default=None, help="also dump a machine-readable result")
    cmp_.set_defaults(func=cmd_compare)

    upd = sub.add_parser("update", help="(re)generate the baseline from a fresh run")
    upd.add_argument("--criterion-dir", action="append", dest="criterion_dirs",
                     metavar="DIR",
                     help="Criterion output dir; repeat once per benchmark repeat "
                          "(the fastest median per benchmark wins). "
                          "Default: target/criterion")
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
