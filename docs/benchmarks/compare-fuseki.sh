#!/usr/bin/env bash
# Orchestrate the same-hardware HTTP head-to-head: Open Triplestore vs Apache
# Jena Fuseki on one Docker network, identical dataset + queries.
#
# Prereqs: `ots-builder` image built (docker build --target builder -t ots-builder .)
#          and data.nt generated (./gen-data.sh > data.nt) in THIS directory.
# Usage:   bash compare-fuseki.sh
set -euo pipefail
CMP="$(cd "$(dirname "$0")" && pwd)"
[ -f "$CMP/data.nt" ] || { echo "missing data.nt — run: ./gen-data.sh > data.nt"; exit 1; }

docker network create otsnet >/dev/null 2>&1 || true
docker rm -f fuseki otssrv >/dev/null 2>&1 || true
trap 'docker rm -f fuseki otssrv >/dev/null 2>&1 || true; docker network rm otsnet >/dev/null 2>&1 || true' EXIT

docker run -d --name fuseki --network otsnet -e ADMIN_PASSWORD=admin -e FUSEKI_DATASET_1=ds stain/jena-fuseki >/dev/null
docker run -d --name otssrv --network otsnet -v "$CMP:/data" ots-builder \
  /app/target/release/open-triplestore --bind 0.0.0.0 --port 7878 \
  --data-dir /tmp/otsd --jwt-secret benchbenchbenchbenchbenchbench32 >/dev/null
docker run --rm --network otsnet -v "$CMP:/data" ots-builder bash /data/compare-client.sh
