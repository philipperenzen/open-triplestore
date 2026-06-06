#!/usr/bin/env bash
# In-container client for the Fuseki head-to-head. Runs on the `otsnet` Docker
# network with /data mounted (containing data.nt). Loads both servers, then times
# identical compute-bound queries, pacing requests below Open Triplestore's
# per-IP SPARQL rate limit so 429s never corrupt the latency measurement.
set -uo pipefail
OURS=http://otssrv:7878; FUSEKI=http://fuseki:3030; D=/data; RUNS=9; WARM=2
jget(){ sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p" | head -1; }

for i in $(seq 1 120); do curl -fsS -o /dev/null "$FUSEKI/\$/ping" 2>/dev/null && break; sleep 1; done
for i in $(seq 1 240); do curl -fsS -o /dev/null "$OURS/sparql?query=ASK%7B%7D" 2>/dev/null && break; sleep 1; done

# Load Fuseki (GSP, admin auth) and Open Triplestore (register → public dataset → GSP).
curl -s -u admin:admin -X PUT -H "Content-Type: application/n-triples" --data-binary @"$D/data.nt" "$FUSEKI/ds/data?default" -o /dev/null -w "fuseki load t=%{time_total}s\n"
REG=$(curl -s -X POST "$OURS/api/auth/register" -H "Content-Type: application/json" -d '{"username":"benchadmin","email":"b@bench.co","password":"benchpass123"}')
TOK=$(printf '%s' "$REG" | jget access_token); A="Authorization: Bearer $TOK"
ORG=$(curl -s -X POST "$OURS/api/organisations" -H "$A" -H "Content-Type: application/json" -d '{"name":"Bench","slug":"bench"}' | jget id)
DS=$(curl -s -X POST "$OURS/api/datasets" -H "$A" -H "Content-Type: application/json" -d "{\"name\":\"BenchDS\",\"owner_type\":\"organisation\",\"owner_id\":\"$ORG\",\"visibility\":\"public\"}" | jget id)
curl -s -X POST "$OURS/api/datasets/$DS/graphs" -H "$A" -H "Content-Type: application/json" -d '{"graph_iri":"http://bench/g"}' -o /dev/null
curl -s -X PUT -H "$A" -H "Content-Type: application/n-triples" --data-binary @"$D/data.nt" "$OURS/store?graph=http%3A%2F%2Fbench%2Fg" -o /dev/null -w "ours load t=%{time_total}s\n"

med(){ sort -n | awk '{a[NR]=$1*1000} END{ if(NR==0){print "n/a";exit} if(NR%2)printf "%.1f",a[(NR+1)/2]; else printf "%.1f",(a[NR/2]+a[NR/2+1])/2 }'; }
paced(){ for r in $(seq 1 $RUNS); do sleep 1.2; curl -s -o /dev/null -w "%{time_total}\n" -G --data-urlencode "query=$2" "$1"; done | med; }

declare -A QF
QF[count]='SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }'
QF[join_count]='SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/name> ?n . ?s <http://example.org/age> ?a }'
QF[group_by]='SELECT ?t (COUNT(?s) AS ?c) (AVG(?a) AS ?avg) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a } GROUP BY ?t'
QF[filter_count]='SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/age> ?a FILTER(?a >= 40 && ?a < 60) }'
QF[distinct]='SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE { ?s <http://example.org/type> ?t }'

printf "%-14s %10s %10s %9s\n" query ours_ms fuseki_ms ratio
for k in count join_count group_by filter_count distinct; do
  qf="${QF[$k]}"; qo="${qf/WHERE/FROM <http://bench/g> WHERE}"   # pin ours to the single bench graph
  for w in $(seq 1 $WARM); do sleep 1.2; curl -s -o /dev/null -G --data-urlencode "query=$qo" "$OURS/sparql"; curl -s -o /dev/null -G --data-urlencode "query=$qf" "$FUSEKI/ds/query"; done
  o=$(paced "$OURS/sparql" "$qo"); fu=$(paced "$FUSEKI/ds/query" "$qf")
  ra=$(awk -v a="$o" -v b="$fu" 'BEGIN{ if(a>0) printf "%.1fx", b/a; else print "n/a" }')
  printf "%-14s %10s %10s %9s\n" "$k" "$o" "$fu" "$ra"
done
