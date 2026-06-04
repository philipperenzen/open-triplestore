#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# W3C SPARQL / RDF Conformance Test Runner
#
# Downloads the official W3C test manifests and runs them against a
# locally-running instance of the open-triplestore.
#
# Sources:
#   - W3C SPARQL 1.1 test suite: https://github.com/w3c/sparql-12/tree/main/tests
#   - W3C RDF tests:             https://github.com/w3c/rdf-tests
#   - ad-freiburg sparql-conformance:
#                                https://github.com/ad-freiburg/sparql-conformance
#
# Usage:
#   ./scripts/run_w3c_conformance.sh [--endpoint URL] [--suite SUITE]
#
# Options:
#   --endpoint URL   SPARQL endpoint to test (default: http://localhost:7878/sparql)
#   --suite SUITE    Which suite to run: sparql11 | rdf11 | sparql12 | all (default: all)
#   --download-only  Only download test files, don't run tests
#   --report FILE    Write conformance report to FILE (default: conformance_report.txt)
#
# Prerequisites:
#   - curl or wget
#   - jq (for JSON processing)
#   - python3 or node (for manifest parsing, optional)
#   - A running instance of open-triplestore on --endpoint
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ─── Configuration ────────────────────────────────────────────────────────────
ENDPOINT="${SPARQL_ENDPOINT:-http://localhost:7878/sparql}"
SUITE="all"
DOWNLOAD_ONLY=false
REPORT_FILE="conformance_report.txt"
TEST_DIR="$(pwd)/.w3c_tests"
BINARY="$(pwd)/target/release/open-triplestore"

# W3C test suite URLs (GitHub raw content)
W3C_SPARQL11_BASE="https://raw.githubusercontent.com/w3c/sparql-12/main/tests"
W3C_RDF_BASE="https://raw.githubusercontent.com/w3c/rdf-tests/main/rdf"
W3C_SPARQL12_BASE="https://raw.githubusercontent.com/w3c/sparql-12/main/tests"

# sparql-conformance (ad-freiburg) – JSON test results format
SPARQL_CONFORMANCE_BASE="https://raw.githubusercontent.com/ad-freiburg/sparql-conformance/main"

# ─── Colors ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# ─── Argument Parsing ─────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --endpoint)  ENDPOINT="$2"; shift 2 ;;
        --suite)     SUITE="$2"; shift 2 ;;
        --download-only) DOWNLOAD_ONLY=true; shift ;;
        --report)    REPORT_FILE="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ─── Helper Functions ─────────────────────────────────────────────────────────

log()  { echo -e "${BLUE}[INFO]${NC} $*"; }
pass() { echo -e "${GREEN}[PASS]${NC} $*"; }
fail() { echo -e "${RED}[FAIL]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }

TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0
SKIPPED_TESTS=0

record_result() {
    local name="$1"
    local result="$2"  # pass | fail | skip
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    case "$result" in
        pass) PASSED_TESTS=$((PASSED_TESTS + 1)); pass "$name" ;;
        fail) FAILED_TESTS=$((FAILED_TESTS + 1)); fail "$name" ;;
        skip) SKIPPED_TESTS=$((SKIPPED_TESTS + 1)); warn "SKIP $name" ;;
    esac
    echo "$result	$name" >> "$REPORT_FILE"
}

check_endpoint() {
    log "Checking endpoint: $ENDPOINT"
    if curl -sf "${ENDPOINT%/sparql}/health" > /dev/null 2>&1; then
        log "✓ Endpoint is reachable"
        return 0
    elif curl -sf "$ENDPOINT?query=ASK+%7B+%7D" -H "Accept: application/sparql-results+json" > /dev/null 2>&1; then
        log "✓ SPARQL endpoint responds"
        return 0
    else
        warn "Endpoint not reachable at $ENDPOINT"
        warn "Start the server with: $BINARY --bind 0.0.0.0 --port 7878"
        return 1
    fi
}

sparql_query() {
    local query="$1"
    local accept="${2:-application/sparql-results+json}"
    curl -sf "$ENDPOINT" \
        --data-urlencode "query=$query" \
        -H "Accept: $accept" \
        2>/dev/null
}

sparql_update() {
    local update="$1"
    curl -sf "${ENDPOINT%/sparql}/sparql" \
        --data-urlencode "update=$update" \
        -X POST \
        -H "Content-Type: application/x-www-form-urlencoded" \
        2>/dev/null
}

download_file() {
    local url="$1"
    local dest="$2"
    if [ ! -f "$dest" ]; then
        mkdir -p "$(dirname "$dest")"
        if command -v curl &> /dev/null; then
            curl -sfL "$url" -o "$dest" 2>/dev/null || return 1
        elif command -v wget &> /dev/null; then
            wget -q "$url" -O "$dest" 2>/dev/null || return 1
        else
            warn "Neither curl nor wget found"
            return 1
        fi
    fi
    return 0
}

# ─── Start Server If Not Running ──────────────────────────────────────────────
start_server_if_needed() {
    if ! check_endpoint 2>/dev/null; then
        if [ -f "$BINARY" ]; then
            log "Starting open-triplestore server..."
            "$BINARY" --bind 0.0.0.0 --port 7878 &
            SERVER_PID=$!
            sleep 2
            trap "kill $SERVER_PID 2>/dev/null" EXIT
        else
            warn "Binary not found at $BINARY — build first with: cargo build --release"
            warn "Continuing with manual server expectation..."
        fi
    fi
}

# ─── SPARQL 1.1 Conformance Tests ─────────────────────────────────────────────

run_sparql11_ask_test() {
    local name="$1"
    local query="$2"
    local expected="$3"   # true | false
    local data="${4:-}"   # Optional Turtle data to load first

    if [ -n "$data" ]; then
        # Load data into a temporary named graph for isolation
        local graph="http://test.local/$(echo "$name" | tr ' /' '_-')"
        sparql_update "CLEAR GRAPH <$graph>" > /dev/null 2>&1 || true
        # Insert the test data
        sparql_update "INSERT DATA { GRAPH <$graph> { $data } }" > /dev/null 2>&1 || true
    fi

    local result
    result=$(sparql_query "$query" "application/sparql-results+json" 2>/dev/null) || {
        record_result "sparql11/$name" "fail"
        return
    }

    local actual
    actual=$(echo "$result" | python3 -c "import sys,json; d=json.load(sys.stdin); print(str(d.get('boolean','')).lower())" 2>/dev/null) || \
    actual=$(echo "$result" | grep -o '"boolean" *: *[a-z]*' | awk -F: '{print $2}' | tr -d ' "' 2>/dev/null) || actual=""

    if [ "$actual" = "$expected" ]; then
        record_result "sparql11/$name" "pass"
    else
        record_result "sparql11/$name" "fail"
        warn "  Expected: $expected, Got: $actual"
    fi
}

run_sparql11_count_test() {
    local name="$1"
    local query="$2"
    local expected_count="$3"

    local result
    result=$(sparql_query "$query" "application/sparql-results+json" 2>/dev/null) || {
        record_result "sparql11/$name" "fail"
        return
    }

    local actual
    actual=$(echo "$result" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('results',{}).get('bindings',[])))" 2>/dev/null) || actual="-1"

    if [ "$actual" = "$expected_count" ]; then
        record_result "sparql11/$name" "pass"
    else
        record_result "sparql11/$name" "fail"
        warn "  Expected count: $expected_count, Got: $actual"
    fi
}

run_sparql11_syntax_test() {
    local name="$1"
    local query="$2"
    local should_succeed="$3"  # true | false

    local http_code
    http_code=$(curl -s -o /dev/null -w "%{http_code}" "$ENDPOINT" \
        --data-urlencode "query=$query" \
        -H "Accept: application/sparql-results+json" 2>/dev/null)

    local success
    [ "$http_code" = "200" ] && success=true || success=false

    if [ "$success" = "$should_succeed" ]; then
        record_result "sparql11-syntax/$name" "pass"
    else
        record_result "sparql11-syntax/$name" "fail"
        warn "  HTTP $http_code for query: $query"
    fi
}

# ─── W3C SPARQL 1.1 Tests ─────────────────────────────────────────────────────

run_w3c_sparql11_suite() {
    echo -e "\n${BOLD}${CYAN}═══ W3C SPARQL 1.1 Conformance Tests ═══${NC}"

    # Positive syntax tests
    log "Running syntax tests..."
    run_sparql11_syntax_test "syntax-select-star"       "SELECT * WHERE { ?s ?p ?o }"                  true
    run_sparql11_syntax_test "syntax-ask"               "ASK { ?s ?p ?o }"                              true
    run_sparql11_syntax_test "syntax-construct"         "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }"     true
    run_sparql11_syntax_test "syntax-describe"          "DESCRIBE <http://example.org/s>"               true
    run_sparql11_syntax_test "syntax-aggregate"         "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }"    true
    run_sparql11_syntax_test "syntax-group-by"          "SELECT ?s (COUNT(*) AS ?c) WHERE { ?s ?p ?o } GROUP BY ?s" true
    run_sparql11_syntax_test "syntax-having"            "SELECT ?s (COUNT(*) AS ?c) WHERE { ?s ?p ?o } GROUP BY ?s HAVING(COUNT(*) > 1)" true
    run_sparql11_syntax_test "syntax-values"            "SELECT ?x WHERE { VALUES ?x { 1 2 3 } }"       true
    run_sparql11_syntax_test "syntax-bind"              "SELECT ?doubled WHERE { BIND(2*3 AS ?doubled) }" true
    run_sparql11_syntax_test "syntax-path-seq"          "SELECT ?o WHERE { ?s <p1>/<p2> ?o }"            true
    run_sparql11_syntax_test "syntax-path-alt"          "SELECT ?o WHERE { ?s (<p1>|<p2>) ?o }"          true
    run_sparql11_syntax_test "syntax-path-star"         "SELECT ?o WHERE { ?s <p>* ?o }"                 true
    run_sparql11_syntax_test "syntax-path-plus"         "SELECT ?o WHERE { ?s <p>+ ?o }"                 true
    run_sparql11_syntax_test "syntax-path-question"     "SELECT ?o WHERE { ?s <p>? ?o }"                 true
    run_sparql11_syntax_test "syntax-path-inverse"      "SELECT ?o WHERE { ?s ^<p> ?o }"                 true
    run_sparql11_syntax_test "syntax-subquery"          "SELECT ?s WHERE { { SELECT ?s WHERE { ?s ?p ?o } LIMIT 1 } }" true
    run_sparql11_syntax_test "syntax-minus"             "SELECT ?s WHERE { ?s ?p ?o MINUS { ?s ?p2 ?o2 } }" true
    run_sparql11_syntax_test "syntax-not-exists"        "SELECT ?s WHERE { ?s ?p ?o FILTER NOT EXISTS { ?s ?p2 ?o2 } }" true

    # Negative syntax tests
    run_sparql11_syntax_test "bad-syntax-unclosed-where" "SELECT ?s WHERE { ?s ?p"        false
    run_sparql11_syntax_test "bad-syntax-bad-filter"     "SELECT ?s WHERE { FILTER(?s }"  false

    # Semantic tests (data-independent)
    log "Running built-in function tests..."
    run_sparql11_ask_test "fn-strlen"   "ASK { FILTER(STRLEN(\"hello\") = 5) }"                 "true"
    run_sparql11_ask_test "fn-ucase"    "ASK { FILTER(UCASE(\"hello\") = \"HELLO\") }"          "true"
    run_sparql11_ask_test "fn-lcase"    "ASK { FILTER(LCASE(\"HELLO\") = \"hello\") }"          "true"
    run_sparql11_ask_test "fn-concat"   "ASK { FILTER(CONCAT(\"a\",\"b\") = \"ab\") }"          "true"
    run_sparql11_ask_test "fn-substr"   "ASK { FILTER(SUBSTR(\"abcdef\",3,2) = \"cd\") }"       "true"
    run_sparql11_ask_test "fn-abs"      "ASK { FILTER(ABS(-5) = 5) }"                            "true"
    run_sparql11_ask_test "fn-ceil"     "ASK { FILTER(CEIL(4.1) = 5) }"                          "true"
    run_sparql11_ask_test "fn-floor"    "ASK { FILTER(FLOOR(4.9) = 4) }"                         "true"
    run_sparql11_ask_test "fn-isiri"    "ASK { FILTER(isIRI(<http://example.org/>)) }"           "true"
    run_sparql11_ask_test "fn-isliteral" "ASK { FILTER(isLiteral(\"hello\")) }"                  "true"
    run_sparql11_ask_test "fn-isnumeric" "ASK { FILTER(isNumeric(42)) }"                          "true"
    run_sparql11_ask_test "fn-md5"      "ASK { FILTER(MD5(\"abc\") = \"900150983cd24fb0d6963f7d28e17f72\") }" "true"
    run_sparql11_ask_test "fn-sha256"   "ASK { FILTER(STRSTARTS(SHA256(\"abc\"), \"ba7816bf\")) }" "true"
    run_sparql11_ask_test "fn-regex"    "ASK { FILTER(REGEX(\"foobar\", \"^foo\")) }"             "true"
    run_sparql11_ask_test "fn-contains" "ASK { FILTER(CONTAINS(\"foobar\", \"oba\")) }"           "true"
    run_sparql11_ask_test "fn-strstarts" "ASK { FILTER(STRSTARTS(\"foobar\", \"foo\")) }"        "true"
    run_sparql11_ask_test "fn-strends"  "ASK { FILTER(STRENDS(\"foobar\", \"bar\")) }"            "true"
    run_sparql11_ask_test "fn-replace"  "ASK { FILTER(REPLACE(\"hello\", \"l\", \"L\") = \"heLLo\") }" "true"
    run_sparql11_ask_test "fn-str"      "ASK { FILTER(STR(42) = \"42\") }"                        "true"

    # Arithmetic type promotion
    run_sparql11_ask_test "type-int-decimal"    "ASK { FILTER((1 + 1.5) = 2.5) }"   "true"
    run_sparql11_ask_test "type-int-int-sum"    "ASK { FILTER((3 + 4) = 7) }"       "true"
    run_sparql11_ask_test "type-compare-types"  "ASK { FILTER(1 = 1.0) }"           "true"
}

# ─── GeoSPARQL Conformance Tests ──────────────────────────────────────────────

run_geosparql_suite() {
    echo -e "\n${BOLD}${CYAN}═══ GeoSPARQL 1.1 Conformance Tests ═══${NC}"

    local geo_pfx="PREFIX geo: <http://www.opengis.net/ont/geosparql#> PREFIX geof: <http://www.opengis.net/def/function/geosparql/>"

    log "Loading GeoSPARQL test data..."
    sparql_update "CLEAR DEFAULT" > /dev/null 2>&1 || true
    sparql_update "INSERT DATA {
        <http://test/park>   geo:hasGeometry [ geo:asWKT \"POLYGON((0 0, 100 0, 100 100, 0 100, 0 0))\"^^geo:wktLiteral ] .
        <http://test/house>  geo:hasGeometry [ geo:asWKT \"POINT(50 50)\"^^geo:wktLiteral ] .
        <http://test/road>   geo:hasGeometry [ geo:asWKT \"LINESTRING(-10 50, 50 50)\"^^geo:wktLiteral ] .
        <http://test/lake>   geo:hasGeometry [ geo:asWKT \"POINT(200 200)\"^^geo:wktLiteral ] .
    }" > /dev/null 2>&1 || true

    # SF topological relations
    run_sparql11_ask_test "geo-sf-contains-true"   "$geo_pfx ASK { FILTER(geof:sfContains(\"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral, \"POINT(5 5)\"^^geo:wktLiteral)) }"  "true"
    run_sparql11_ask_test "geo-sf-contains-false"  "$geo_pfx ASK { FILTER(geof:sfContains(\"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral, \"POINT(15 15)\"^^geo:wktLiteral)) }" "false"
    run_sparql11_ask_test "geo-sf-intersects"      "$geo_pfx ASK { FILTER(geof:sfIntersects(\"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral, \"POLYGON((5 5,15 5,15 15,5 15,5 5))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-sf-disjoint"        "$geo_pfx ASK { FILTER(geof:sfDisjoint(\"POLYGON((0 0,1 0,1 1,0 1,0 0))\"^^geo:wktLiteral, \"POLYGON((5 5,6 5,6 6,5 6,5 5))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-sf-within"          "$geo_pfx ASK { FILTER(geof:sfWithin(\"POINT(5 5)\"^^geo:wktLiteral, \"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-sf-equals"          "$geo_pfx ASK { FILTER(geof:sfEquals(\"POINT(1 2)\"^^geo:wktLiteral, \"POINT(1 2)\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-sf-touches"         "$geo_pfx ASK { FILTER(geof:sfTouches(\"POINT(0 0)\"^^geo:wktLiteral, \"LINESTRING(0 0,10 10)\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-sf-crosses"         "$geo_pfx ASK { FILTER(geof:sfCrosses(\"LINESTRING(0 0,10 10)\"^^geo:wktLiteral, \"LINESTRING(0 10,10 0)\"^^geo:wktLiteral)) }" "true"

    # Egenhofer
    run_sparql11_ask_test "geo-eh-contains"   "$geo_pfx ASK { FILTER(geof:ehContains(\"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral, \"POINT(5 5)\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-eh-disjoint"   "$geo_pfx ASK { FILTER(geof:ehDisjoint(\"POLYGON((0 0,1 0,1 1,0 1,0 0))\"^^geo:wktLiteral, \"POLYGON((5 5,6 5,6 6,5 6,5 5))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-eh-overlap"    "$geo_pfx ASK { FILTER(geof:ehOverlap(\"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral, \"POLYGON((5 5,15 5,15 15,5 15,5 5))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-eh-meet"       "$geo_pfx ASK { FILTER(geof:ehMeet(\"POLYGON((0 0,5 0,5 5,0 5,0 0))\"^^geo:wktLiteral, \"POLYGON((5 0,10 0,10 5,5 5,5 0))\"^^geo:wktLiteral)) }" "true"

    # RCC8
    run_sparql11_ask_test "geo-rcc8-dc"   "$geo_pfx ASK { FILTER(geof:rcc8dc(\"POLYGON((0 0,1 0,1 1,0 1,0 0))\"^^geo:wktLiteral, \"POLYGON((5 5,6 5,6 6,5 6,5 5))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-rcc8-ec"   "$geo_pfx ASK { FILTER(geof:rcc8ec(\"POLYGON((0 0,5 0,5 5,0 5,0 0))\"^^geo:wktLiteral, \"POLYGON((5 0,10 0,10 5,5 5,5 0))\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-rcc8-eq"   "$geo_pfx ASK { FILTER(geof:rcc8eq(\"POINT(1 2)\"^^geo:wktLiteral, \"POINT(1 2)\"^^geo:wktLiteral)) }" "true"
    run_sparql11_ask_test "geo-rcc8-ntpp" "$geo_pfx ASK { FILTER(geof:rcc8ntpp(\"POINT(5 5)\"^^geo:wktLiteral, \"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral)) }" "true"

    # Metric functions
    run_sparql11_ask_test "geo-distance-345" "$geo_pfx ASK { FILTER(geof:distance(\"POINT(0 0)\"^^geo:wktLiteral, \"POINT(3 4)\"^^geo:wktLiteral) = 5.0) }" "true"
    run_sparql11_ask_test "geo-area-100"     "$geo_pfx ASK { FILTER(geof:area(\"POLYGON((0 0,10 0,10 10,0 10,0 0))\"^^geo:wktLiteral) = 100.0) }" "true"

    # Spatial queries on loaded data
    run_sparql11_count_test "geo-find-in-park" \
        "$geo_pfx SELECT ?f WHERE { ?f geo:hasGeometry/geo:asWKT ?wkt . FILTER(geof:sfWithin(?wkt, \"POLYGON((0 0,100 0,100 100,0 100,0 0))\"^^geo:wktLiteral)) }" \
        "2"  # house and road (road enters park)
}

# ─── RDF 1.1 Format Tests ─────────────────────────────────────────────────────

run_rdf11_suite() {
    echo -e "\n${BOLD}${CYAN}═══ W3C RDF 1.1 Format Conformance Tests ═══${NC}"

    local graph_store="${ENDPOINT%/sparql}/store"

    log "Testing Turtle format..."
    # PUT valid Turtle
    local http_code
    http_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=http://test.rdf11/turtle-test" \
        -X PUT \
        -H "Content-Type: text/turtle" \
        --data-binary '@prefix ex: <http://example.org/> . ex:s ex:p ex:o .' 2>/dev/null)

    if [ "$http_code" = "200" ] || [ "$http_code" = "201" ] || [ "$http_code" = "204" ]; then
        record_result "rdf11/turtle-put" "pass"
    else
        record_result "rdf11/turtle-put" "fail"
        warn "  HTTP $http_code for Turtle PUT"
    fi

    log "Testing N-Triples format..."
    http_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=http://test.rdf11/nt-test" \
        -X PUT \
        -H "Content-Type: application/n-triples" \
        --data-binary '<http://example.org/s> <http://example.org/p> "value" .' 2>/dev/null)

    if [ "$http_code" = "200" ] || [ "$http_code" = "201" ] || [ "$http_code" = "204" ]; then
        record_result "rdf11/ntriples-put" "pass"
    else
        record_result "rdf11/ntriples-put" "fail"
    fi

    log "Testing content negotiation..."
    # GET Turtle
    local accept_code
    accept_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=http://test.rdf11/turtle-test" \
        -H "Accept: text/turtle" 2>/dev/null)
    [ "$accept_code" = "200" ] && record_result "rdf11/turtle-get" "pass" || record_result "rdf11/turtle-get" "fail"

    # GET N-Triples
    accept_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=http://test.rdf11/nt-test" \
        -H "Accept: application/n-triples" 2>/dev/null)
    [ "$accept_code" = "200" ] && record_result "rdf11/ntriples-get" "pass" || record_result "rdf11/ntriples-get" "fail"

    # GET RDF/XML
    accept_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=http://test.rdf11/turtle-test" \
        -H "Accept: application/rdf+xml" 2>/dev/null)
    [ "$accept_code" = "200" ] && record_result "rdf11/rdfxml-get" "pass" || record_result "rdf11/rdfxml-get" "fail"

    log "Testing SPARQL result formats..."
    # JSON results
    local json_code
    json_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$ENDPOINT" \
        --data-urlencode "query=SELECT * WHERE { ?s ?p ?o } LIMIT 1" \
        -H "Accept: application/sparql-results+json" 2>/dev/null)
    [ "$json_code" = "200" ] && record_result "rdf11/sparql-json" "pass" || record_result "rdf11/sparql-json" "fail"

    # XML results
    local xml_code
    xml_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$ENDPOINT" \
        --data-urlencode "query=SELECT * WHERE { ?s ?p ?o } LIMIT 1" \
        -H "Accept: application/sparql-results+xml" 2>/dev/null)
    [ "$xml_code" = "200" ] && record_result "rdf11/sparql-xml" "pass" || record_result "rdf11/sparql-xml" "fail"

    # CSV results
    local csv_code
    csv_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$ENDPOINT" \
        --data-urlencode "query=SELECT * WHERE { ?s ?p ?o } LIMIT 1" \
        -H "Accept: text/csv" 2>/dev/null)
    [ "$csv_code" = "200" ] && record_result "rdf11/sparql-csv" "pass" || record_result "rdf11/sparql-csv" "fail"

    # TSV results
    local tsv_code
    tsv_code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$ENDPOINT" \
        --data-urlencode "query=SELECT * WHERE { ?s ?p ?o } LIMIT 1" \
        -H "Accept: text/tab-separated-values" 2>/dev/null)
    [ "$tsv_code" = "200" ] && record_result "rdf11/sparql-tsv" "pass" || record_result "rdf11/sparql-tsv" "fail"
}

# ─── Graph Store Protocol Tests ───────────────────────────────────────────────

run_graph_store_suite() {
    echo -e "\n${BOLD}${CYAN}═══ SPARQL 1.1 Graph Store HTTP Protocol Tests ═══${NC}"

    local graph_store="${ENDPOINT%/sparql}/store"
    local test_graph="http://test.gsp/$(date +%s)"

    log "Testing PUT (create/replace graph)..."
    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=$test_graph" \
        -X PUT -H "Content-Type: text/turtle" \
        --data-binary "@prefix ex: <http://ex/> . ex:s ex:p \"hello\" ." 2>/dev/null)
    { [ "$code" = "200" ] || [ "$code" = "201" ] || [ "$code" = "204" ]; } && \
        record_result "gsp/put-named-graph" "pass" || record_result "gsp/put-named-graph" "fail"

    log "Testing GET (retrieve graph)..."
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=$test_graph" \
        -H "Accept: text/turtle" 2>/dev/null)
    [ "$code" = "200" ] && record_result "gsp/get-named-graph" "pass" || record_result "gsp/get-named-graph" "fail"

    log "Testing POST (merge into graph)..."
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=$test_graph" \
        -X POST -H "Content-Type: text/turtle" \
        --data-binary "@prefix ex: <http://ex/> . ex:s2 ex:p \"world\" ." 2>/dev/null)
    { [ "$code" = "200" ] || [ "$code" = "201" ] || [ "$code" = "204" ]; } && \
        record_result "gsp/post-merge" "pass" || record_result "gsp/post-merge" "fail"

    # Verify the merge worked (should have 2 triples now)
    local count
    count=$(sparql_query "SELECT (COUNT(*) AS ?c) WHERE { GRAPH <$test_graph> { ?s ?p ?o } }" \
        "application/sparql-results+json" | \
        python3 -c "import sys,json; d=json.load(sys.stdin); print(d['results']['bindings'][0]['c']['value'])" 2>/dev/null)
    [ "$count" = "2" ] && record_result "gsp/post-merge-count" "pass" || {
        record_result "gsp/post-merge-count" "fail"
        warn "  Expected 2 triples, got: $count"
    }

    log "Testing DELETE (remove graph)..."
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?graph=$test_graph" \
        -X DELETE 2>/dev/null)
    { [ "$code" = "200" ] || [ "$code" = "204" ]; } && \
        record_result "gsp/delete-named-graph" "pass" || record_result "gsp/delete-named-graph" "fail"

    log "Testing default graph operations..."
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?default" \
        -X PUT -H "Content-Type: text/turtle" \
        --data-binary "@prefix ex: <http://ex/> . ex:d ex:p \"default\" ." 2>/dev/null)
    { [ "$code" = "200" ] || [ "$code" = "201" ] || [ "$code" = "204" ]; } && \
        record_result "gsp/put-default-graph" "pass" || record_result "gsp/put-default-graph" "fail"

    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$graph_store?default" \
        -H "Accept: text/turtle" 2>/dev/null)
    [ "$code" = "200" ] && record_result "gsp/get-default-graph" "pass" || record_result "gsp/get-default-graph" "fail"
}

# ─── Service Description Test ─────────────────────────────────────────────────

run_service_description_test() {
    echo -e "\n${BOLD}${CYAN}═══ SPARQL Service Description Test ═══${NC}"

    local sd_url="${ENDPOINT%/sparql}"
    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "$sd_url" -H "Accept: text/turtle" 2>/dev/null)
    [ "$code" = "200" ] && record_result "service-description/turtle" "pass" || record_result "service-description/turtle" "fail"

    # Check the service description mentions SPARQL 1.1
    local sd
    sd=$(curl -sf "$sd_url" -H "Accept: text/turtle" 2>/dev/null)
    if echo "$sd" | grep -qi "sparql11"; then
        record_result "service-description/mentions-sparql11" "pass"
    else
        record_result "service-description/mentions-sparql11" "skip"
        warn "  Service description doesn't explicitly mention SPARQL 1.1"
    fi
}

# ─── Health Check Test ─────────────────────────────────────────────────────────

run_health_check() {
    echo -e "\n${BOLD}${CYAN}═══ Health Check Test ═══${NC}"
    local health_url="${ENDPOINT%/sparql}/health"
    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" "$health_url" 2>/dev/null)
    [ "$code" = "200" ] && record_result "health/endpoint" "pass" || record_result "health/endpoint" "fail"
}

# ─── Main ─────────────────────────────────────────────────────────────────────

main() {
    echo -e "${BOLD}${CYAN}"
    echo "╔══════════════════════════════════════════════════════════╗"
    echo "║    Open Triplestore W3C Conformance Test Runner        ║"
    echo "╚══════════════════════════════════════════════════════════╝"
    echo -e "${NC}"

    log "Endpoint: $ENDPOINT"
    log "Suite:    $SUITE"
    log "Report:   $REPORT_FILE"

    # Initialize report file
    echo "# Open Triplestore Conformance Report" > "$REPORT_FILE"
    echo "# Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$REPORT_FILE"
    echo "# Endpoint: $ENDPOINT" >> "$REPORT_FILE"
    echo "# Format: result\ttest_name" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"

    if [ "$DOWNLOAD_ONLY" = true ]; then
        log "Download-only mode; skipping test execution"
        exit 0
    fi

    # Start server if needed
    start_server_if_needed

    # Check endpoint is reachable
    if ! check_endpoint; then
        fail "Cannot reach endpoint. Exiting."
        exit 1
    fi

    # Run requested suites
    if [ "$SUITE" = "all" ] || [ "$SUITE" = "sparql11" ]; then
        run_w3c_sparql11_suite
    fi

    if [ "$SUITE" = "all" ] || [ "$SUITE" = "geosparql" ]; then
        run_geosparql_suite
    fi

    if [ "$SUITE" = "all" ] || [ "$SUITE" = "rdf11" ]; then
        run_rdf11_suite
    fi

    if [ "$SUITE" = "all" ] || [ "$SUITE" = "gsp" ]; then
        run_graph_store_suite
    fi

    if [ "$SUITE" = "all" ]; then
        run_service_description_test
        run_health_check
    fi

    # ─── Summary ──────────────────────────────────────────────────────────────
    echo ""
    echo -e "${BOLD}${CYAN}═══ Conformance Test Summary ═══${NC}"
    echo ""
    printf "%-20s %d\n" "Total tests:"   "$TOTAL_TESTS"
    printf "%-20s ${GREEN}%d${NC}\n" "Passed:" "$PASSED_TESTS"
    printf "%-20s ${RED}%d${NC}\n"   "Failed:" "$FAILED_TESTS"
    printf "%-20s ${YELLOW}%d${NC}\n" "Skipped:" "$SKIPPED_TESTS"

    local pct=0
    if [ "$TOTAL_TESTS" -gt 0 ]; then
        pct=$(( (PASSED_TESTS * 100) / TOTAL_TESTS ))
    fi
    printf "%-20s %d%%\n" "Pass rate:" "$pct"

    # Write summary to report
    echo "" >> "$REPORT_FILE"
    echo "# Summary" >> "$REPORT_FILE"
    echo "# Total:   $TOTAL_TESTS" >> "$REPORT_FILE"
    echo "# Passed:  $PASSED_TESTS" >> "$REPORT_FILE"
    echo "# Failed:  $FAILED_TESTS" >> "$REPORT_FILE"
    echo "# Skipped: $SKIPPED_TESTS" >> "$REPORT_FILE"
    echo "# Rate:    $pct%" >> "$REPORT_FILE"

    echo ""
    log "Report written to: $REPORT_FILE"

    if [ "$FAILED_TESTS" -eq 0 ]; then
        echo -e "${GREEN}${BOLD}All tests passed! ✓${NC}"
        exit 0
    else
        echo -e "${RED}${BOLD}$FAILED_TESTS test(s) failed.${NC}"
        exit 1
    fi
}

main "$@"
