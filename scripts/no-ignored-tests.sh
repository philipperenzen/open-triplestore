#!/usr/bin/env bash
# No test is ignored. An `#[ignore]` — plain, with a reason, or behind a
# `cfg_attr` for one platform — runs in no job, so it is a deleted test that
# still looks like one. Fix the test, or delete it and say why in the commit.
# CI runs this next to `cargo test`, whose summary must also report 0 ignored
# (that catches ignored doctests, which this source scan does not look for).
set -euo pipefail
cd "$(dirname "$0")/.."
hits=$(grep -rnE --include='*.rs' \
  -e '#\[ignore' \
  -e 'cfg_attr\(.*\bignore\b' \
  -e '^[[:space:]]*ignore[[:space:]]*(=[[:space:]]*"|\)|,)' \
  src tests plugins tools opengraph benches 2>/dev/null \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
if [ -n "$hits" ]; then
  echo "Ignored tests — fix or delete them:"
  echo "$hits"
  exit 1
fi
echo "No ignored tests."
