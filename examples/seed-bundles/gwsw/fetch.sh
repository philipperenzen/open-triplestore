#!/usr/bin/env bash
# Fetch the GWSW Totaal Turtle export next to manifest.toml. RIONED serves
# every sub-model as https://data.gwsw.nl/<version>/<Module>/ontologie.ttl
# (CC0, no login); Totaal is the combined one. Override GWSW_TTL_URL for
# another version or module (e.g. .../1.7.0/Basis/ontologie.ttl).
set -euo pipefail
cd "$(dirname "$0")"
GWSW_TTL_URL="${GWSW_TTL_URL:-https://data.gwsw.nl/1.7.0/Totaal/ontologie.ttl}"
echo "→ gwsw-totaal.ttl from $GWSW_TTL_URL"; curl -fsSL "$GWSW_TTL_URL" -o gwsw-totaal.ttl
head -c 200 gwsw-totaal.ttl; echo
