#!/usr/bin/env bash
# Download the NEN 2660-2 RDFS model next to manifest.toml. Run once; the
# payload is git-ignored. Publisher: DigiGO (gh-pages of nl-digigo/nen2660),
# downloads page https://nl-digigo.github.io/nen2660/-/downloads/
set -euo pipefail
cd "$(dirname "$0")"
NEN=https://raw.githubusercontent.com/nl-digigo/nen2660/gh-pages/data
echo "→ nen2660-rdfs.ttl"; curl -fsSL "$NEN/nen2660-rdfs.ttl" -o nen2660-rdfs.ttl
echo "done — the bundle loads at boot from --seed-dir (SEED_DIR) pointing at examples/seed-bundles"
