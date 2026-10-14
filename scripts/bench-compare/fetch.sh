#!/usr/bin/env bash
# Fetch the third-party inputs of the store comparison and generate the data.
#
#   scripts/bench-compare/fetch.sh <work-dir> [product counts...]   (default: 1000 10000)
#
# Writes into <work-dir>:
#   bsbmtools-0.2/                 the BSBM tools (GPL-3.0; fetched, never vendored)
#   cache/jena-fuseki-server-6.2.0.jar   Apache Jena Fuseki main (Apache-2.0)
#   data/bsbm-<scale>/dataset.nt   BSBM N-Triples, generated with forward chaining (-fc)
#   data/bsbm-<scale>/params.json  the seeded explore-mix query instances (bench.py params)
#
# Every download is checked against the SHA-256 recorded below. The generator is
# deterministic: the same product count gives a byte-identical dataset (the
# hashes of the 1k and 10k files are in docs/perf-data/<date>/inputs.json).
set -euo pipefail
WORK="${1:?usage: fetch.sh <work-dir> [product counts...]}"
shift
if [ "$#" -eq 0 ]; then COUNTS=(1000 10000); else COUNTS=("$@"); fi
HERE="$(cd "$(dirname "$0")" && pwd)"
JDK_IMAGE="eclipse-temurin:21-jdk@sha256:3e3c176ffed168beb42c607be9bc1639b466cf00261a0fb04425562c9d0c5c2b"

BSBM_URL="https://sourceforge.net/projects/bsbmtools/files/bsbmtools/bsbmtools-0.2/bsbmtools-v0.2.zip/download"
BSBM_SHA256="40f5e59baadec3af0014b7647989d3e0fc0476af25e84a4bc9d7f8cd81520aaa"
FUSEKI_URL="https://repo1.maven.org/maven2/org/apache/jena/jena-fuseki-server/6.2.0/jena-fuseki-server-6.2.0.jar"
FUSEKI_SHA256="2c92c598e65ab69d99820052e05f4277ed1d77980a8845f87545ad35b71a76ee"

check() { # file sha256
  local got
  got=$(shasum -a 256 "$1" | cut -d' ' -f1)
  [ "$got" = "$2" ] || { echo "checksum mismatch for $1: $got (expected $2)" >&2; exit 1; }
}

mkdir -p "$WORK/cache" "$WORK/data"
if [ ! -d "$WORK/bsbmtools-0.2" ]; then
  curl -fsSL -o "$WORK/bsbmtools-v0.2.zip" "$BSBM_URL"
  check "$WORK/bsbmtools-v0.2.zip" "$BSBM_SHA256"
  (cd "$WORK" && unzip -q bsbmtools-v0.2.zip)
fi
if [ ! -f "$WORK/cache/jena-fuseki-server-6.2.0.jar" ]; then
  curl -fsSL -o "$WORK/cache/jena-fuseki-server-6.2.0.jar" "$FUSEKI_URL"
fi
check "$WORK/cache/jena-fuseki-server-6.2.0.jar" "$FUSEKI_SHA256"

for n in "${COUNTS[@]}"; do
  scale="$((n / 1000))k"
  out="$WORK/data/bsbm-$scale"
  mkdir -p "$out"
  if [ ! -s "$out/dataset.nt" ]; then
    # The generator reads its word lists from the working directory.
    docker run --rm -v "$WORK/bsbmtools-0.2:/bsbm:ro" -v "$out:/out" -w /bsbm "$JDK_IMAGE" \
      java -Xmx2g -cp '/bsbm/lib/*' benchmark.generator.Generator -fc -pc "$n" -s nt \
      -fn /out/dataset -dir /out/td_data
  fi
  python3 "$HERE/bench.py" params --data "$out/dataset.nt" --bsbm "$WORK/bsbmtools-0.2" --out "$out/params.json"
  shasum -a 256 "$out/dataset.nt"
done
