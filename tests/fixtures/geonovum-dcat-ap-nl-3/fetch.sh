#!/usr/bin/env bash
# Fetch the DCAT-AP-NL 3 SHACL shapes from Geonovum's repository at a pinned
# commit and check each file's sha256. The repository has no LICENSE file, so
# the shapes are fetched for the test run, never committed (see README.md).
# tests/dcat_conformance.rs validates the catalogue against them when they are
# here; CI runs this script first and sets OTS_TEST_DCAT_AP_NL_REQUIRED=1 so a
# missing file fails the run instead of skipping it.
set -euo pipefail
cd "$(dirname "$0")"
COMMIT=5106d18720fbe6ad56905f43e87fa9fe544104a7
BASE="https://raw.githubusercontent.com/Geonovum/DCAT-AP-NL30/$COMMIT/shapes"
if command -v sha256sum >/dev/null; then SHA="sha256sum"; else SHA="shasum -a 256"; fi
while read -r sha file; do
  [ -z "$file" ] && continue
  if [ -f "$file" ] && echo "$sha  $file" | $SHA -c --status 2>/dev/null; then
    echo "✓ $file (cached)"; continue
  fi
  curl -fsSL "$BASE/$file" -o "$file.part"
  if ! echo "$sha  $file.part" | $SHA -c --status; then
    echo "✗ $file: sha256 mismatch (upstream changed at the pinned commit?)" >&2
    rm -f "$file.part"; exit 1
  fi
  mv "$file.part" "$file"; echo "✓ $file"
done <<'PINS'
bde0965ce8c0a97be3cf236dbe55b174d7d9b3646d08575edd6310a397c4e655 dcat-ap-nl-SHACL.ttl
e0b0cf701cb121f413fb3af941544fe3c913012544a169fd706d1a1c0882f894 dcat-ap-nl-SHACL-klassebereik.ttl
55729a634d00820debee78f60d5ff6e4beb5cff99fa1544b2de2add38caa6a4d dcat-ap-nl-SHACL-klassebereik-codelijsten.ttl
b7f4f0bce2578c6f632fe963accc29fdfd2ac06170c1cf6944eb95d6bada6504 dcat-ap-nl-SHACL-aanbevolen.ttl
PINS
