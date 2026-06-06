#!/usr/bin/env bash
# ============================================================================
# 3-way SPARQL HTTP head-to-head: Open Triplestore vs Apache Jena Fuseki (TDB2)
# vs QLever, on the identical 501k-triple gen_persons dataset.
#
# Produces the table in docs/performance.md ("Comparison with Apache Jena Fuseki
# and QLever"). Latency is the median of N warm runs (one untimed warmup first,
# which builds Open Triplestore's in-memory mirror), counting only HTTP 200s.
#
# Prerequisites: Docker, curl, awk. An Open Triplestore **release** server must be
# reachable at $OTS_BASE (build it first, e.g.:
#   docker run -d --name ots -p 7878:7878 -e JWT_SECRET=$(openssl rand -hex 32) \
#     -e RATE_LIMIT_DISABLED=1 <your open-triplestore image> \
#     --data-dir /data --bind 0.0.0.0
# RATE_LIMIT_DISABLED=1 makes the measurement reflect the engine, not the limiter.)
#
# Fuseki and QLever are started here as throwaway containers and removed at the end.
# ============================================================================
set -u
WORK="${WORK:-$(mktemp -d)}"
OTS_BASE="${OTS_BASE:-http://localhost:7878}"
FUSEKI_PORT="${FUSEKI_PORT:-3030}"
QLEVER_PORT="${QLEVER_PORT:-7001}"
N_PERSONS="${N_PERSONS:-167000}"   # x3 triples = 501k
RUNS="${RUNS:-9}"
PACE="${PACE:-0.1}"
EX="http://example.org/"
cd "$WORK"
echo "workdir: $WORK"

# ---- 1. generate data.nt (gen_persons: name/age/type per person) ----
awk -v ex="$EX" -v n="$N_PERSONS" 'BEGIN{
  xsd="http://www.w3.org/2001/XMLSchema#integer";
  for(i=0;i<n;i++){
    printf "<%sp%d> <%sname> \"Person %d\" .\n",ex,i,ex,i;
    printf "<%sp%d> <%sage> \"%d\"^^<%s> .\n",ex,i,ex,18+i%65,xsd;
    printf "<%sp%d> <%stype> <%sType%d> .\n",ex,i,ex,ex,i%10;
  }}' > data.nt
echo "generated $(wc -l < data.nt) triples"

# ---- 2. Open Triplestore: register a public dataset, PUT data to its graph ----
TOKEN=$(curl -s -X POST "$OTS_BASE/api/auth/register" -H "Content-Type: application/json" \
  -d '{"username":"bench","email":"bench@local","password":"benchpass12345"}' \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
[ -z "$TOKEN" ] && TOKEN=$(curl -s -X POST "$OTS_BASE/api/auth/login" -H "Content-Type: application/json" \
  -d '{"username":"bench","password":"benchpass12345"}' | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
AID=$(curl -s -H "Authorization: Bearer $TOKEN" "$OTS_BASE/api/auth/me" | sed -n 's/.*"\(id\|user_id\)":"\([^"]*\)".*/\2/p' | head -1)
curl -s -X POST "$OTS_BASE/api/datasets" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d "{\"name\":\"Bench\",\"owner_type\":\"user\",\"owner_id\":\"$AID\",\"visibility\":\"public\"}" >/dev/null
GRAPH="$OTS_BASE/dataset/bench/data"; ENC=$(printf %s "$GRAPH" | sed 's#:#%3A#g;s#/#%2F#g')
curl -s -X POST "$OTS_BASE/api/datasets/bench/graphs" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d "{\"graph_iri\":\"$GRAPH\"}" >/dev/null
curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/n-triples" \
  --data-binary @data.nt "$OTS_BASE/store?graph=$ENC" -o /dev/null
OTS="$OTS_BASE/api/datasets/bench/services/sparql/sparql"

# ---- 3. Fuseki (TDB2) ----
docker rm -f fuseki.bench >/dev/null 2>&1
docker run -d --name fuseki.bench -p "$FUSEKI_PORT:3030" -e ADMIN_PASSWORD=admin stain/jena-fuseki >/dev/null
for _ in $(seq 1 30); do curl -sf -u admin:admin "http://localhost:$FUSEKI_PORT/\$/ping" >/dev/null 2>&1 && break; sleep 1; done
curl -s -X POST -u admin:admin "http://localhost:$FUSEKI_PORT/\$/datasets?dbType=tdb2&dbName=ds" >/dev/null
curl -s -X PUT -u admin:admin -H "Content-Type: application/n-triples" --data-binary @data.nt "http://localhost:$FUSEKI_PORT/ds?default" -o /dev/null
FUSEKI="http://localhost:$FUSEKI_PORT/ds/query"

# ---- 4. QLever (index then serve) ----
docker rm -f qlever.bench >/dev/null 2>&1
mkdir -p qindex && cp data.nt qindex/
docker run --rm -v "$WORK/qindex:/index" --entrypoint bash adfreiburg/qlever \
  -c "cd /index && /qlever/qlever-index -F ttl -f data.nt -i data" >/dev/null 2>&1
docker run -d --name qlever.bench -p "$QLEVER_PORT:7001" -v "$WORK/qindex:/index" --entrypoint bash adfreiburg/qlever \
  -c "/qlever/qlever-server -i /index/data -p 7001 -m 2G" >/dev/null
for _ in $(seq 1 30); do curl -sf "http://localhost:$QLEVER_PORT/?cmd=stats" >/dev/null 2>&1 && break; sleep 1; done
QLEVER="http://localhost:$QLEVER_PORT"

# ---- 5. timed comparison ----
q_ots()    { curl -s -o /dev/null -w "%{http_code} %{time_total}" "$OTS"    --data-urlencode "query=$1" -H "Accept: text/tab-separated-values"; }
q_fuseki() { curl -s -o /dev/null -w "%{http_code} %{time_total}" "$FUSEKI" --data-urlencode "query=$1" -H "Accept: text/tab-separated-values"; }
q_qlever() { curl -s -o /dev/null -w "%{http_code} %{time_total}" "$QLEVER" -H "Content-type: application/sparql-query" --data "$1" -H "Accept: text/tab-separated-values"; }
median_ms() { # $1=fn $2=query — median of HTTP-200 runs only
  local fn="$1" q="$2" code t times=()
  $fn "$q" >/dev/null 2>&1; sleep "$PACE"
  for _ in $(seq 1 "$RUNS"); do read -r code t < <($fn "$q" 2>/dev/null); [ "$code" = "200" ] && times+=("$t"); sleep "$PACE"; done
  [ "${#times[@]}" -lt 5 ] && { printf "ERR"; return; }
  printf '%s\n' "${times[@]}" | sort -n | awk -v n="${#times[@]}" 'NR==int((n+1)/2){printf "%.1f", $1*1000}'
}
NAMES=("COUNT(*)" "2-way join COUNT" "FILTER + COUNT" "GROUP BY + COUNT" "GROUP BY + AVG" "COUNT(DISTINCT)")
QUERIES=(
  "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }"
  "SELECT (COUNT(*) AS ?c) WHERE { ?s <${EX}name> ?n . ?s <${EX}age> ?a }"
  "SELECT (COUNT(*) AS ?c) WHERE { ?s <${EX}age> ?a FILTER(?a >= 40 && ?a < 60) }"
  "SELECT ?t (COUNT(*) AS ?c) WHERE { ?s <${EX}type> ?t } GROUP BY ?t"
  "SELECT ?t (AVG(?a) AS ?avg) WHERE { ?s <${EX}type> ?t . ?s <${EX}age> ?a } GROUP BY ?t"
  "SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE { ?s <${EX}type> ?t }"
)
printf "\n| %-22s | %12s | %12s | %12s |\n" "Query (~501k, HTTP)" "OpenTS (ms)" "Fuseki (ms)" "QLever (ms)"
printf "|%s|%s|%s|%s|\n" "------------------------" "--------------" "--------------" "--------------"
for i in "${!QUERIES[@]}"; do
  printf "| %-22s | %12s | %12s | %12s |\n" "${NAMES[$i]}" \
    "$(median_ms q_ots "${QUERIES[$i]}")" "$(median_ms q_fuseki "${QUERIES[$i]}")" "$(median_ms q_qlever "${QUERIES[$i]}")"
done

# ---- 6. cleanup (leaves the OTS server running) ----
docker rm -f fuseki.bench qlever.bench >/dev/null 2>&1
echo; echo "done (Fuseki/QLever containers removed; OTS at $OTS_BASE left running)"
