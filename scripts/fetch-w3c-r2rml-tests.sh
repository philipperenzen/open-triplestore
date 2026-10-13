#!/usr/bin/env bash
# Fetch the W3C R2RML test cases (RDB2RDF Working Group) for
# tests/w3c_r2rml_conformance.rs.
#
# They are fetched, never committed. W3C publishes its test suites under the
# W3C test-suite licences (https://www.w3.org/copyright/test-suites-licenses/);
# whether the 3-clause BSD option covers this suite is unconfirmed, so the
# files stay out of the repository, and no score is published from them
# (W3C's policy allows no public performance claims on its test suites).
#
# Source: the copy the W3C Knowledge Graph Construction Community Group keeps
# with per-database SQL scripts (PostgreSQL and MySQL variants), pinned to a
# commit. Every file is checked against scripts/w3c-r2rml-tests.sha256, so a
# moved or altered upstream file fails the fetch instead of the tests.
#
#   scripts/fetch-w3c-r2rml-tests.sh [DEST]   # default tests/fixtures/w3c-r2rml
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO="kg-construct/r2rml-test-cases-support"
COMMIT="976098802e9bb0e297d196b26a4c2e0b95fd7644"
SUMS="$ROOT/scripts/w3c-r2rml-tests.sha256"
DEST="${1:-$ROOT/tests/fixtures/w3c-r2rml}"

if command -v sha256sum >/dev/null 2>&1; then
  hash() { sha256sum "$1" | cut -d' ' -f1; }
else
  hash() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

mkdir -p "$DEST"
n=0
while read -r sum path; do
  [ -z "$path" ] && continue
  out="$DEST/$path"
  mkdir -p "$(dirname "$out")"
  if [ ! -f "$out" ] || [ "$(hash "$out")" != "$sum" ]; then
    curl -fsSL --retry 3 "https://raw.githubusercontent.com/$REPO/$COMMIT/$path" -o "$out"
  fi
  got="$(hash "$out")"
  if [ "$got" != "$sum" ]; then
    echo "sha256 mismatch for $path: expected $sum, got $got" >&2
    rm -f "$out"
    exit 1
  fi
  n=$((n + 1))
done < "$SUMS"
echo "→ $n files of $REPO@${COMMIT:0:12} in $DEST"
