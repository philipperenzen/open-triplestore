#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════
# perf_selftest.sh — validate scripts/perf_regression.py against fixtures.
#
# Runs entirely on hand-written Criterion fixtures under scripts/testdata/, so it
# needs NO native build and NO multi-minute benchmark run — it works on Windows
# Git-Bash, macOS, Linux, and CI. Invoked by `make perf-check-selftest`.
# ═══════════════════════════════════════════════════════════════════════════
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
script="$here/perf_regression.py"
fix="$here/testdata/criterion_fixture"
base_ok="$here/testdata/baseline_fixture.json"
base_regress="$here/testdata/baseline_fixture_regress.json"

# Pick an interpreter that actually RUNS — on Windows `python3` is often a Microsoft
# Store stub that's on PATH but errors, so probe execution rather than mere presence.
PY="${PYTHON:-}"
if [ -n "$PY" ] && ! "$PY" -c 'import sys' >/dev/null 2>&1; then PY=""; fi
if [ -z "$PY" ]; then
  for cand in python3 python py; do
    if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import sys' >/dev/null 2>&1; then PY="$cand"; break; fi
  done
fi
if [ -z "$PY" ]; then echo "FAIL: no working python3/python/py on PATH" >&2; exit 1; fi

fails=0
expect_exit() { # <expected> <label> -- <cmd...>
  local want="$1" label="$2"; shift 3
  local out got
  set +e; out="$("$@" 2>&1)"; got=$?; set -e
  if [ "$got" -eq "$want" ]; then
    echo "PASS: $label (exit $got)"
  else
    echo "FAIL: $label — expected exit $want, got $got"; echo "$out"; fails=$((fails + 1))
  fi
  LAST_OUT="$out"
}

echo "== perf_regression.py self-test (python: $PY) =="

# 1. Pass case: medians match baseline -> exit 0, with two soft warnings.
expect_exit 0 "pass case (matching medians)" -- \
  "$PY" "$script" check --criterion-dir "$fix" --baseline "$base_ok"
case "$LAST_OUT" in
  *"999 ns"*) echo "FAIL: base/estimates.json was read (found 999 ns) — only new/ must be parsed"; fails=$((fails + 1));;
  *) echo "PASS: base/ ignored (no 999 ns in output)";;
esac
case "$LAST_OUT" in
  *"not in baseline"*) echo "PASS: missing-from-baseline soft warning present";;
  *) echo "FAIL: expected a 'not in baseline' warning"; fails=$((fails + 1));;
esac

# 2. Regression case: 2 regressions, concurrent suppressed by prefix tolerance -> exit 1.
expect_exit 1 "regression case" -- \
  "$PY" "$script" check --criterion-dir "$fix" --baseline "$base_regress"
case "$LAST_OUT" in
  *"concurrent/throughput/8"*"REGRESSION"*) echo "FAIL: concurrent flagged despite 1.5x prefix tolerance"; fails=$((fails + 1));;
  *) echo "PASS: concurrent/ prefix tolerance suppressed its 1.43x delta";;
esac

# 3. Empty results -> operational error exit 2.
empty="$(mktemp -d)"; trap 'rm -rf "$empty"' EXIT
expect_exit 2 "empty criterion dir" -- \
  "$PY" "$script" check --criterion-dir "$empty" --baseline "$base_ok"

# 4. update writes a populated, sorted baseline -> exit 0.
out_json="$(mktemp)"; trap 'rm -rf "$empty"; rm -f "$out_json"' EXIT
expect_exit 0 "update writes baseline" -- \
  "$PY" "$script" update --criterion-dir "$fix" --out "$out_json" --runner selftest
if "$PY" -c "import json,sys; d=json.load(open(sys.argv[1])); assert d['benchmarks']['query/simple_lookup/1000']==275000.0; assert d['schema_version']==1; assert d['generator']['runner']=='selftest'" "$out_json"; then
  echo "PASS: update produced a valid populated baseline"
else
  echo "FAIL: update output malformed"; fails=$((fails + 1))
fi

# 5. `compare` — the PR gate's mode: a change against its merge base, both
#    measured here. Two passes a side, and the FASTEST median per benchmark wins,
#    so a single slow pass on either side must not swing the verdict.
cmp_root="$(mktemp -d)"; trap 'rm -rf "$empty" "$cmp_root"; rm -f "$out_json"' EXIT
for side in base-1 base-2 head-1 head-2; do mkdir -p "$cmp_root/$side/b/new"; done
write_median() { printf '{"median":{"point_estimate":%s}}' "$2" > "$cmp_root/$1/b/new/estimates.json"; }
cmp_base="$(mktemp)"; trap 'rm -rf "$empty" "$cmp_root"; rm -f "$out_json" "$cmp_base"' EXIT
printf '{"schema_version":1,"default_tolerance_ratio":1.10,"tolerances":{},"benchmarks":{}}' > "$cmp_base"

# Base 1000 ns (one pass polluted to 1400), change 1050 ns (one pass to 1500):
# fastest-of-two gives 1050/1000 = 1.05, inside +10%.
write_median base-1 1000.0; write_median base-2 1400.0
write_median head-1 1050.0; write_median head-2 1500.0
expect_exit 0 "compare: fastest-of-two ignores a polluted pass on each side" -- \
  "$PY" "$script" compare --before "$cmp_root/base-1" --before "$cmp_root/base-2" \
                          --after "$cmp_root/head-1" --after "$cmp_root/head-2" \
                          --baseline "$cmp_base"

# Same base, change genuinely 30% slower in BOTH passes -> regression.
write_median head-1 1300.0; write_median head-2 1300.0
expect_exit 1 "compare: real regression on both passes fails" -- \
  "$PY" "$script" compare --before "$cmp_root/base-1" --before "$cmp_root/base-2" \
                          --after "$cmp_root/head-1" --after "$cmp_root/head-2" \
                          --baseline "$cmp_base"

# A missing side is an operational error, never a silent pass.
expect_exit 2 "compare: empty 'after' side" -- \
  "$PY" "$script" compare --before "$cmp_root/base-1" --after "$empty" --baseline "$cmp_base"

echo "===================================="
if [ "$fails" -eq 0 ]; then echo "ALL PERF SELF-TESTS PASSED"; else echo "$fails PERF SELF-TEST(S) FAILED"; exit 1; fi
