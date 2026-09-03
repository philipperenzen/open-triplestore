#!/usr/bin/env bash
# Download the NEN 2660-2 and IMBOR 2025 RDF next to manifest.toml. Run once;
# the payloads are git-ignored. Publishers: DigiGO (NEN 2660-2, gh-pages of
# nl-digigo/nen2660) and CROW (IMBOR, GitHub release ZIP).
#   https://nl-digigo.github.io/nen2660/-/downloads/
#   https://github.com/Stichting-CROW/imbor/releases
set -euo pipefail
cd "$(dirname "$0")"
NEN=https://raw.githubusercontent.com/nl-digigo/nen2660/gh-pages/data
for f in nen2660-skos.ttl nen2660-rdfs.ttl nen2660-owl.ttl nen2660-shacl.ttl; do
  echo "→ $f"; curl -fsSL "$NEN/$f" -o "$f"
done
# IMBOR 2025 Linked Data release (≈4.3 MB ZIP; the TTL files inside are what
# manifest.toml points at, under ./imbor-release/). Override IMBOR_ZIP for
# another release.
IMBOR_ZIP="${IMBOR_ZIP:-https://github.com/Stichting-CROW/imbor/releases/download/2025/IMBOR-2025.LinkedData.zipfile.zip}"
echo "→ IMBOR release zip"; curl -fsSL "$IMBOR_ZIP" -o imbor.zip
unzip -o -q imbor.zip -d imbor-release
ls -1 imbor-release
echo "done — the bundle loads at boot from --seed-dir (SEED_DIR) pointing at examples/seed-bundles"
