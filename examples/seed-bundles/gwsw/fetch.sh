#!/usr/bin/env bash
# Fetch the GWSW Totaal Turtle export next to manifest.toml. RIONED publishes
# the ontology through https://data.gwsw.nl/ and the apps portal; obtain the
# download URL there and run:  GWSW_TTL_URL=<url> ./fetch.sh
set -euo pipefail
cd "$(dirname "$0")"
: "${GWSW_TTL_URL:?set GWSW_TTL_URL to the GWSW Totaal Turtle download URL (see https://apps.gwsw.nl/)}"
echo "→ gwsw-totaal.ttl"; curl -fsSL "$GWSW_TTL_URL" -o gwsw-totaal.ttl
head -c 200 gwsw-totaal.ttl; echo
