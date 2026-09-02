#!/usr/bin/env bash
# Download the NEN 2660-2 and IMBOR RDF next to manifest.toml. Run once; the
# files are git-ignored. Confirm the file names on the publishers' pages if a
# download 404s — they are maintained by DigiGO (NEN 2660) and CROW (IMBOR):
#   https://nl-digigo.github.io/nen2660/-/downloads/
#   https://github.com/Stichting-CROW/imbor/releases
set -euo pipefail
cd "$(dirname "$0")"
NEN=https://raw.githubusercontent.com/nl-digigo/nen2660/gh-pages/data
for f in nen2660-term.ttl nen2660-rdfs.ttl nen2660-owl.ttl nen2660-shacl.ttl; do
  echo "→ $f"; curl -fsSL "$NEN/$f" -o "$f"
done
# IMBOR: the object-type library ships inside the release ZIP. Set IMBOR_ZIP to
# the release asset URL from the releases page, e.g.
#   IMBOR_ZIP=https://github.com/Stichting-CROW/imbor/releases/download/2025/imbor-2025-rdf.zip ./fetch.sh
if [ -n "${IMBOR_ZIP:-}" ]; then
  echo "→ IMBOR release zip"; curl -fsSL "$IMBOR_ZIP" -o imbor.zip
  unzip -o -q imbor.zip -d imbor-release
  echo "Extracted to ./imbor-release — copy the object-type library TTL to imbor-otl.ttl"
  echo "and its SHACL shapes to imbor-shapes.ttl (see manifest.toml)."
else
  echo "IMBOR_ZIP not set — skipping IMBOR (the bundle needs imbor-otl.ttl and imbor-shapes.ttl)."
fi
