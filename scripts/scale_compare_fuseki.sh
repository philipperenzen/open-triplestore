#!/usr/bin/env bash
# Run the HTTP benchmark driver (scripts/scale_compare_http.py) against Apache
# Jena Fuseki on the OTL-shaped dataset. Fuseki "main" (no UI, no Shiro) from
# the Maven Central jar is the fair choice on any architecture:
#
#   FUSEKI_JAR=~/Downloads/jena-fuseki-server-6.2.0.jar \
#     scripts/scale_compare_fuseki.sh otl-100k.ttl [port]
#
# Without FUSEKI_JAR the stain/jena-fuseki Docker image is used (amd64 only —
# emulated and slow on arm64 hosts, so its numbers are not comparable).
set -euo pipefail
FILE="${1:?usage: scale_compare_fuseki.sh <turtle-file> [port]}"
PORT="${2:-3031}"
NAME=ots-scale-fuseki
LOC=$(mktemp -d /tmp/ots-fuseki-tdb2.XXXXXX)
cleanup() { if [ -n "${FUSEKI_JAR:-}" ]; then kill "${FUSEKI_PID:-0}" >/dev/null 2>&1 || true; else docker rm -f "$NAME" >/dev/null 2>&1 || true; fi; rm -rf "$LOC"; }
trap cleanup EXIT
if [ -n "${FUSEKI_JAR:-}" ]; then
  mkdir -p "$LOC/tdb"   # Fuseki refuses to create the TDB2 directory itself
  java -Xmx4g -jar "$FUSEKI_JAR" --port "$PORT" --update --tdb2 --loc "$LOC/tdb" /otl > "$LOC/fuseki.log" 2>&1 &
  FUSEKI_PID=$!
else
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker run -d --name "$NAME" -p "$PORT:3030" -e ADMIN_PASSWORD=pw -e JVM_ARGS=-Xmx4g stain/jena-fuseki >/dev/null
fi
for i in $(seq 1 90); do curl -sf "http://127.0.0.1:$PORT/$/ping" >/dev/null 2>&1 && break; sleep 1; done
curl -sf "http://127.0.0.1:$PORT/$/ping" >/dev/null || { echo "fuseki did not come up" >&2; tail -20 "$LOC/fuseki.log" >&2 2>/dev/null || true; exit 1; }
if [ -z "${FUSEKI_JAR:-}" ]; then
  curl -sf -u admin:pw -X POST "http://127.0.0.1:$PORT/$/datasets" -d "dbName=otl&dbType=tdb2" >/dev/null
fi
python3 "$(dirname "$0")/scale_compare_http.py" --name "fuseki $( [ -n "${FUSEKI_JAR:-}" ] && echo main || echo docker ) tdb2" \
  --sparql "http://127.0.0.1:$PORT/otl/sparql" --update "http://127.0.0.1:$PORT/otl/update" --gsp "http://127.0.0.1:$PORT/otl/data" \
  --file "$FILE" "${@:3}"
