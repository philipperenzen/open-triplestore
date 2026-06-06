#!/usr/bin/env bash
# ============================================================================
# 3-way SPARQL HTTP head-to-head: Open Triplestore vs Apache Jena Fuseki (TDB2)
# vs QLever, on the identical 501k-triple gen_persons dataset.
#
# Prints a verbose per-query summary to the terminal AND auto-generates a markdown
# report (default ./benchmark-report.md, override with REPORT=...) with the system
# environment, the full results table with per-query speedup ratios, and copy-paste
# reproduction steps — so it is easy to run and reproduce on any device. The curated
# numbers live in docs/performance.md ("Comparison with Apache Jena Fuseki and QLever").
# Latency is the median of $RUNS warm runs (one untimed warm-up first, which builds
# Open Triplestore's in-memory mirror), counting only HTTP 200s.
#
# Prerequisites: Docker, curl, awk. An Open Triplestore **release** server must be
# reachable at $OTS_BASE (build it first, e.g.:
#   docker run -d --name ots -p 7878:7878 -e JWT_SECRET=$(openssl rand -hex 32) \
#     -e RATE_LIMIT_DISABLED=1 -e OTS_QUERY_CACHE=0 <your open-triplestore image> \
#     --data-dir /data --bind 0.0.0.0
# RATE_LIMIT_DISABLED=1 and OTS_QUERY_CACHE=0 make the numbers reflect cold engine
# compute, not the rate limiter or warm cache hits.)
#
# Env knobs: OTS_BASE, N_PERSONS (dataset size), RUNS (samples/query), REPORT (path).
# Fuseki and QLever are started here as throwaway containers and removed at the end.
# ============================================================================
set -u
ORIG_PWD="$(pwd)"
WORK="${WORK:-$(mktemp -d)}"
OTS_BASE="${OTS_BASE:-http://localhost:7878}"
FUSEKI_PORT="${FUSEKI_PORT:-3030}"
QLEVER_PORT="${QLEVER_PORT:-7001}"
N_PERSONS="${N_PERSONS:-167000}"   # x3 triples = 501k
RUNS="${RUNS:-9}"
PACE="${PACE:-0.1}"
EX="http://example.org/"
# Where the auto-generated markdown report is written (override with REPORT=...).
REPORT="${REPORT:-$ORIG_PWD/benchmark-report.md}"
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
# Throwaway credentials for the local benchmark instance, generated per run (never
# hardcoded — keeps secret scanners happy and avoids a reusable literal).
PW="${BENCH_PASSWORD:-$(openssl rand -hex 12)}"
TOKEN=$(curl -s -X POST "$OTS_BASE/api/auth/register" -H "Content-Type: application/json" \
  -d "{\"username\":\"bench\",\"email\":\"bench@local\",\"password\":\"$PW\"}" \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
[ -z "$TOKEN" ] && TOKEN=$(curl -s -X POST "$OTS_BASE/api/auth/login" -H "Content-Type: application/json" \
  -d "{\"username\":\"bench\",\"password\":\"$PW\"}" | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
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
NAMES=("COUNT(*)" "2-way join COUNT" "FILTER + COUNT" "GROUP BY + COUNT" \
       "GROUP BY + AVG" "COUNT(DISTINCT)" "GROUP BY + COUNT(DISTINCT)" "global AVG")
QUERIES=(
  "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }"
  "SELECT (COUNT(*) AS ?c) WHERE { ?s <${EX}name> ?n . ?s <${EX}age> ?a }"
  "SELECT (COUNT(*) AS ?c) WHERE { ?s <${EX}age> ?a FILTER(?a >= 40 && ?a < 60) }"
  "SELECT ?t (COUNT(*) AS ?c) WHERE { ?s <${EX}type> ?t } GROUP BY ?t"
  "SELECT ?t (AVG(?a) AS ?avg) WHERE { ?s <${EX}type> ?t . ?s <${EX}age> ?a } GROUP BY ?t"
  "SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE { ?s <${EX}type> ?t }"
  "SELECT ?t (COUNT(DISTINCT ?a) AS ?c) WHERE { ?s <${EX}type> ?t . ?s <${EX}age> ?a } GROUP BY ?t"
  "SELECT (AVG(?a) AS ?avg) WHERE { ?s <${EX}age> ?a }"
)

# Run every query on every engine, capturing the medians so we can both print the
# table and analyse it (winners, speedups) for the report.
declare -a R_OTS R_FUS R_QL
echo
echo "Running ${#QUERIES[@]} queries × 3 engines × ${RUNS} warm runs (median of HTTP-200 only)…"
for i in "${!QUERIES[@]}"; do
  printf "  [%d/%d] %-28s " "$((i + 1))" "${#QUERIES[@]}" "${NAMES[$i]}"
  R_OTS[$i]=$(median_ms q_ots "${QUERIES[$i]}")
  R_FUS[$i]=$(median_ms q_fuseki "${QUERIES[$i]}")
  R_QL[$i]=$(median_ms q_qlever "${QUERIES[$i]}")
  printf "OTS=%-7s Fuseki=%-7s QLever=%-7s\n" "${R_OTS[$i]}" "${R_FUS[$i]}" "${R_QL[$i]}"
done

# a÷b as "x.xx×", or "n/a" if either is non-numeric (ERR).
ratio() { awk -v a="$1" -v b="$2" 'BEGIN{ if (a+0>0 && b+0>0) printf "%.2f×", a/b; else print "n/a" }'; }

emit_table() { # markdown table, used for both the terminal and the report
  printf "| %-28s | %9s | %9s | %9s | %8s | %8s |\n" \
    "Query (~$((N_PERSONS * 3)) triples)" "OpenTS ms" "Fuseki ms" "QLever ms" "Fus/OTS" "OTS/QL"
  printf "|%s|%s|%s|%s|%s|%s|\n" \
    ":-----------------------------" "--------:" "--------:" "--------:" "-------:" "------:"
  for i in "${!QUERIES[@]}"; do
    printf "| %-28s | %9s | %9s | %9s | %8s | %8s |\n" "${NAMES[$i]}" \
      "${R_OTS[$i]}" "${R_FUS[$i]}" "${R_QL[$i]}" \
      "$(ratio "${R_FUS[$i]}" "${R_OTS[$i]}")" "$(ratio "${R_OTS[$i]}" "${R_QL[$i]}")"
  done
}

emit_summary() { # bullet stats over the captured results (robust to sub-ms noise)
  local nf=0 nm=0 nq=0 tot=0 o f q
  for i in "${!QUERIES[@]}"; do
    o="${R_OTS[$i]}"; f="${R_FUS[$i]}"; q="${R_QL[$i]}"
    [ "$(awk -v a="$o" 'BEGIN{print (a+0>0)?1:0}')" = "1" ] || continue
    tot=$((tot + 1))
    [ "$(awk -v o="$o" -v f="$f" 'BEGIN{print (f+0>0 && o+0<f+0)?1:0}')" = "1" ] && nf=$((nf + 1))
    [ "$(awk -v o="$o" -v q="$q" 'BEGIN{print (q+0>0 && o+0<=1.25*(q+0))?1:0}')" = "1" ] && nm=$((nm + 1))
    [ "$(awk -v o="$o" -v q="$q" 'BEGIN{print (q+0>0 && o+0<=2*(q+0))?1:0}')" = "1" ] && nq=$((nq + 1))
  done
  echo "- Beats Apache Jena Fuseki (TDB2) on **${nf}/${tot}** queries."
  echo "- Matches QLever — within **1.25×** — on **${nm}/${tot}**."
  echo "- Within **2×** of QLever on **${nq}/${tot}** (QLever's edge is columnar dictionary IDs + sorted-permutation merge joins)."
}

# ---- 6. environment + report ----
# System facts captured from inside Docker — the actual benchmark environment (on
# macOS/Windows the host differs from Docker's Linux VM, which is what runs the engines).
SYS=$(docker run --rm --entrypoint sh adfreiburg/qlever -c 'echo "CPU=$(grep -m1 "model name" /proc/cpuinfo 2>/dev/null | cut -d: -f2 | sed "s/^ *//")"; echo "CORES=$(nproc)"; echo "MEMKB=$(grep MemTotal /proc/meminfo | tr -dc 0-9)"; echo "KERNEL=$(uname -sr)"' 2>/dev/null)
CPU=$(printf '%s\n' "$SYS" | sed -n 's/^CPU=//p'); [ -z "$CPU" ] && CPU="unknown"
CORES=$(printf '%s\n' "$SYS" | sed -n 's/^CORES=//p')
MEMKB=$(printf '%s\n' "$SYS" | sed -n 's/^MEMKB=//p')
KERNEL=$(printf '%s\n' "$SYS" | sed -n 's/^KERNEL=//p')
MEM=$(awk -v k="${MEMKB:-0}" 'BEGIN{printf "%.1f GiB", k/1048576}')
STAMP=$(date -u +"%Y-%m-%d %H:%M UTC")

# ---- terminal: verbose summary ----
echo
echo "═══════════════════════════════════════════════════════════════════════"
echo "  Open Triplestore vs Fuseki vs QLever — $((N_PERSONS * 3)) triples, over HTTP"
echo "  $STAMP · ${CPU} (${CORES} cores) · ${MEM} · ${KERNEL}"
echo "═══════════════════════════════════════════════════════════════════════"
echo
emit_table
echo
echo "Summary (Fus/OTS = how many × OpenTS beats Fuseki; OTS/QL <1 = OpenTS faster):"
emit_summary
echo

# ---- markdown report ----
{
  echo "# Open Triplestore — engine benchmark report"
  echo
  echo "_Generated: ${STAMP}_"
  echo
  echo "3-way SPARQL-over-HTTP comparison on an identical $((N_PERSONS * 3))-triple \`gen_persons\`"
  echo "dataset (name / age / type per person). Latency is the **median of ${RUNS} warm runs**"
  echo "returning ≤10 rows (so it reflects engine work, not result transfer), HTTP-200 only."
  echo "Open Triplestore is queried through a dataset's SPARQL **service endpoint** (single-graph"
  echo "ACL scope, like Fuseki's \`/ds\` and QLever's single dataset); its per-IP rate limiter and"
  echo "result cache are disabled (\`RATE_LIMIT_DISABLED=1\`, \`OTS_QUERY_CACHE=0\`) so the figures"
  echo "are **cold engine compute**, not warm cache hits."
  echo
  echo "## Environment"
  echo
  echo "- **CPU:** ${CPU} (${CORES} cores visible to Docker)"
  echo "- **RAM:** ${MEM}"
  echo "- **Kernel:** ${KERNEL}"
  echo "- **Engines:** Open Triplestore (this build) · Apache Jena Fuseki (\`stain/jena-fuseki\`, TDB2) · QLever (\`adfreiburg/qlever\`)"
  echo "- **Dataset:** $((N_PERSONS * 3)) triples (${N_PERSONS} persons: name / age / type)"
  echo "- **Method:** median of ${RUNS} warm runs after one untimed warm-up (which builds OpenTS's in-memory mirror)"
  echo
  echo "## Results"
  echo
  emit_table
  echo
  echo "\`Fus/OTS\` = Fuseki ÷ OpenTS (×; >1 ⇒ OpenTS is faster). \`OTS/QL\` = OpenTS ÷ QLever"
  echo "(×; <1 ⇒ OpenTS is faster, >1 ⇒ QLever is faster). \`ERR\`/\`n/a\` ⇒ fewer than 5 HTTP-200s."
  echo
  echo "## Summary"
  echo
  emit_summary
  echo
  echo "## Reproduce"
  echo
  echo '```bash'
  echo '# 1. Start an Open Triplestore *release* server with the cache + rate limiter off'
  echo '#    (so the numbers are engine compute, not cache hits):'
  echo 'docker run -d --name ots -p 7878:7878 \'
  echo '  -e RATE_LIMIT_DISABLED=1 -e OTS_QUERY_CACHE=0 -e JWT_SECRET=$(openssl rand -hex 32) \'
  echo '  <open-triplestore-image> --data-dir /data --bind 0.0.0.0'
  echo '# 2. Run this script — it generates the data, loads all three engines, and spins'
  echo '#    up + removes the Fuseki and QLever containers itself:'
  echo 'OTS_BASE=http://localhost:7878 bash scripts/compare_engines.sh'
  echo '```'
  echo
  echo "_Tunables (env): \`N_PERSONS\` (dataset size), \`RUNS\` (samples/query), \`REPORT\` (this file's path)._"
} >"$REPORT"

echo "Report written to: $REPORT"

# ---- 7. cleanup (leaves the OTS server running) ----
docker rm -f fuseki.bench qlever.bench >/dev/null 2>&1
echo "Done — Fuseki/QLever containers removed; OpenTS left running at $OTS_BASE."
