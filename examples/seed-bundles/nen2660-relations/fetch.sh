#!/usr/bin/env bash
# Download the NEN 2660-2 RDFS model next to manifest.toml. Run once; the
# payload is git-ignored — NEN 2660-2 carries no licence that allows
# redistributing it. Publisher: NEN (gh-pages of
# NEN-Nederlands-Normalisatie-Instituut/nen2660, which took the files over from
# DigiGO's nl-digigo/nen2660).
set -euo pipefail
cd "$(dirname "$0")"
NEN="${NEN_BASE_URL:-https://raw.githubusercontent.com/NEN-Nederlands-Normalisatie-Instituut/nen2660/gh-pages/data}"
echo "→ nen2660-rdfs.ttl"; curl -fsSL "$NEN/nen2660-rdfs.ttl" -o nen2660-rdfs.ttl
echo "done — the bundle loads at boot from --seed-dir (SEED_DIR) pointing at examples/seed-bundles"
