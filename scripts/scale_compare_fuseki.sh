#!/usr/bin/env bash
# Load the same OTL-shaped dataset into Apache Jena Fuseki (Docker) and time
# the load and the benchmark's query shapes over HTTP, for docs/performance.md.
#
#   SCALE_DUMP=/tmp/otl.ttl cargo run --release --example scale_otl -- 100000 /tmp/scale-otl
#   scripts/scale_compare_fuseki.sh /tmp/otl.ttl
# Two ways to run Fuseki: the Docker image (amd64 only — emulated and slow on
# arm64 hosts) or, with FUSEKI_HOME pointing at an unpacked
# apache-jena-fuseki distribution, natively on the host's JVM (fair on any
# architecture; needs Java 17+).
set -euo pipefail
FILE="${1:?usage: scale_compare_fuseki.sh <turtle-file> [port]}"
PORT="${2:-3031}"
NAME=ots-scale-fuseki
LOC=$(mktemp -d /tmp/ots-fuseki-tdb2.XXXXXX)
cleanup() { if [ -n "${FUSEKI_HOME:-}" ]; then kill "${FUSEKI_PID:-0}" >/dev/null 2>&1 || true; else docker rm -f "$NAME" >/dev/null 2>&1 || true; fi; rm -rf "$LOC"; }
trap cleanup EXIT
if [ -n "${FUSEKI_HOME:-}" ]; then
  # The webapp distribution requires a login by default (Shiro). This is a
  # local benchmark: give it a private FUSEKI_BASE whose shiro.ini allows
  # everything, so the load and the queries need no credentials.
  export FUSEKI_BASE="$LOC/base"
  mkdir -p "$FUSEKI_BASE"
  printf '[main]\nssl.enabled = false\n\n[urls]\n/** = anon\n' > "$FUSEKI_BASE/shiro.ini"
  JAVA_OPTS="-Xmx4g" "$FUSEKI_HOME/fuseki-server" --port "$PORT" --update --tdb2 --loc "$LOC/tdb" /otl > "$LOC/fuseki.log" 2>&1 &
  FUSEKI_PID=$!
  for i in $(seq 1 90); do curl -sf "http://127.0.0.1:$PORT/$/ping" >/dev/null 2>&1 && break; sleep 1; done
else
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker run -d --name "$NAME" -p "$PORT:3030" -e ADMIN_PASSWORD=pw -e JVM_ARGS=-Xmx4g stain/jena-fuseki >/dev/null
  for i in $(seq 1 90); do curl -sf "http://127.0.0.1:$PORT/$/ping" >/dev/null 2>&1 && break; sleep 1; done
  curl -sf -u admin:pw -X POST "http://127.0.0.1:$PORT/$/datasets" -d "dbName=otl&dbType=tdb2" >/dev/null
fi
EX=https://example.org/otl/
G=https://example.org/otl/instances
echo "fuseki ready after wait; ping=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/$/ping")" >&2
t0=$(date +%s.%N)
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H 'Content-Type: text/turtle' --data-binary "@$FILE" "http://127.0.0.1:$PORT/otl/data?graph=$G")
t1=$(date +%s.%N)
echo "load PUT: HTTP $code" >&2
if [ "$code" != "200" ] && [ "$code" != "201" ] && [ "$code" != "204" ]; then
  echo "load failed; fuseki log tail:" >&2; tail -30 "$LOC/fuseki.log" >&2 2>/dev/null || true; exit 22
fi
LOAD=$(python3 -c "print(round($t1-$t0,2))")
N=$(curl -s -H 'Accept: application/sparql-results+json' --data-urlencode "query=SELECT (COUNT(*) AS ?c) WHERE { GRAPH <$G> { ?s ?p ?o } }" "http://127.0.0.1:$PORT/otl/sparql" | python3 -c 'import sys,json; print(json.load(sys.stdin)["results"]["bindings"][0]["c"]["value"])' 2>/dev/null || { echo "count query failed" >&2; tail -20 "$LOC/fuseki.log" >&2 2>/dev/null || true; exit 22; })
echo "{\"store\":\"fuseki $( [ -n "${FUSEKI_HOME:-}" ] && echo native || echo docker )\",\"quads\":$N,\"load_seconds\":$LOAD,\"load_quads_per_s\":$(python3 -c "print(round($N/$LOAD))"),\"queries\":{"
ASSETS=$(python3 -c "print(($N+0)//9)")
PROBE=$((ASSETS/2))
run() { # name query
  local name=$1 q=$2 best=""
  for i in 1 2 3 4 5 6; do
    s=$(date +%s.%N); qc=$(curl -s -o /dev/null -w '%{http_code}' -H 'Accept: application/sparql-results+json' --data-urlencode "query=$q" "http://127.0.0.1:$PORT/otl/sparql"); e=$(date +%s.%N)
    [ "$qc" = "200" ] || { echo "query $name: HTTP $qc" >&2; tail -20 "$LOC/fuseki.log" >&2 2>/dev/null || true; exit 22; }
    ms=$(python3 -c "print(($e-$s)*1000)"); [ $i -gt 1 ] && best="$best $ms"
  done
  med=$(python3 -c "v=sorted(map(float,'$best'.split())); print(round(v[len(v)//2],2))")
  echo "  \"$name\": {\"median_ms\": $med},"
}
run lookup "SELECT ?p ?o WHERE { GRAPH <$G> { <${EX}asset$PROBE> ?p ?o } }"
run join_2way "SELECT ?a ?pn WHERE { GRAPH <$G> { ?a <${EX}partOf> ?p . ?p <${EX}name> ?pn } } LIMIT 10000"
run filter "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <$G> { ?a <${EX}length> ?l FILTER(?l > 40.0 && ?l < 42.0) } }"
run group_by "SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) WHERE { GRAPH <$G> { ?a a ?t ; <${EX}length> ?l } } GROUP BY ?t"
run path "SELECT (COUNT(?anc) AS ?c) WHERE { GRAPH <$G> { <${EX}asset$PROBE> <${EX}partOf>+ ?anc } }"
run count_all "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <$G> { ?s ?p ?o } }"
echo '  "_": null } }'
