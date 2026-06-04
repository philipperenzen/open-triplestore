#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════
# Benchmark script for open-triplestore
# Tests insertion and query performance
# ═══════════════════════════════════════════════════════════

set -euo pipefail

ENDPOINT="${1:-http://localhost:7878}"
SPARQL="$ENDPOINT/sparql"
STORE="$ENDPOINT/store"

echo "═══ Local Triple Store Benchmark ═══"
echo "Endpoint: $ENDPOINT"
echo ""

# ─── 1. Health check ───
echo "1. Health check..."
curl -sf "$ENDPOINT/health" | python3 -m json.tool 2>/dev/null || echo "(health check output)"
echo ""

# ─── 2. Generate test data ───
echo "2. Generating test data (10,000 triples)..."
TMPFILE=$(mktemp /tmp/benchmark_data.XXXXXX.ttl)

cat > "$TMPFILE" << 'HEADER'
@prefix ex: <http://example.org/benchmark/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .
@prefix sf: <http://www.opengis.net/ont/sf#> .
HEADER

for i in $(seq 1 10000); do
    lat=$(echo "scale=6; ($RANDOM % 180) - 90" | bc)
    lon=$(echo "scale=6; ($RANDOM % 360) - 180" | bc)
    echo "ex:entity$i ex:name \"Entity $i\" ; ex:value $i ; ex:hasGeometry [ a sf:Point ; geo:asWKT \"POINT($lon $lat)\"^^geo:wktLiteral ] ." >> "$TMPFILE"
done

echo "   Generated: $(wc -l < "$TMPFILE") lines"

# ─── 3. Load test data ───
echo "3. Loading data..."
LOAD_START=$(date +%s%N)
curl -sf -X POST "$STORE?default" \
    -H "Content-Type: text/turtle" \
    --data-binary @"$TMPFILE" \
    -o /dev/null -w "   HTTP %{http_code} in %{time_total}s\n"
LOAD_END=$(date +%s%N)
LOAD_MS=$(( (LOAD_END - LOAD_START) / 1000000 ))
echo "   Load time: ${LOAD_MS}ms"
echo ""

# ─── 4. Query benchmarks ───
echo "4. Running query benchmarks..."
echo ""

run_query() {
    local name="$1"
    local query="$2"
    echo "   $name:"
    START=$(date +%s%N)
    RESULT=$(curl -sf -G "$SPARQL" \
        --data-urlencode "query=$query" \
        -H "Accept: application/sparql-results+json" \
        -w "\n%{http_code} %{time_total}" \
        2>/dev/null)
    END=$(date +%s%N)
    MS=$(( (END - START) / 1000000 ))
    echo "     Time: ${MS}ms"
}

run_query "Simple SELECT (all triples, LIMIT 10)" \
    "SELECT * WHERE { ?s ?p ?o } LIMIT 10"

run_query "COUNT all triples" \
    "SELECT (COUNT(*) as ?count) WHERE { ?s ?p ?o }"

run_query "Filter by value range" \
    "PREFIX ex: <http://example.org/benchmark/> SELECT ?s ?v WHERE { ?s ex:value ?v . FILTER(?v > 9990) } ORDER BY ?v"

run_query "OPTIONAL + FILTER" \
    "PREFIX ex: <http://example.org/benchmark/> SELECT ?s ?name WHERE { ?s ex:name ?name . OPTIONAL { ?s ex:value ?v } FILTER(BOUND(?v) && ?v < 5) }"

run_query "Aggregation GROUP BY" \
    "PREFIX ex: <http://example.org/benchmark/> SELECT (COUNT(?s) as ?count) (AVG(?v) as ?avg) (MAX(?v) as ?max) WHERE { ?s ex:value ?v }"

run_query "GeoSPARQL: spatial contains" \
    "PREFIX geo: <http://www.opengis.net/ont/geosparql#>
     PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
     SELECT ?result WHERE {
       BIND(geof:sfContains(
         \"POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))\"^^geo:wktLiteral,
         \"POINT(5 5)\"^^geo:wktLiteral
       ) AS ?result)
     }"

run_query "GeoSPARQL: distance calculation" \
    "PREFIX geo: <http://www.opengis.net/ont/geosparql#>
     PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
     SELECT ?dist WHERE {
       BIND(geof:distance(
         \"POINT(0 0)\"^^geo:wktLiteral,
         \"POINT(3 4)\"^^geo:wktLiteral
       ) AS ?dist)
     }"

echo ""

# ─── 5. Cleanup ───
rm -f "$TMPFILE"
echo "═══ Benchmark complete ═══"
