#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# Test runner for open-triplestore
#
# Runs all test categories and reports results:
#   1. Unit tests (src/**/*.rs #[test] blocks)
#   2. Original integration tests (tests/integration_test.rs)
#   3. W3C SPARQL 1.1 conformance (tests/w3c_sparql11_conformance.rs)
#      Derived from: https://www.w3.org/2009/sparql/docs/tests/summary.html
#                    https://github.com/ad-freiburg/sparql-conformance
#   4. W3C RDF 1.1 conformance (tests/rdf11_conformance.rs)
#      Derived from: https://github.com/w3c/rdf-tests
#   5. OGC GeoSPARQL conformance (tests/geosparql_conformance.rs)
#      Derived from: https://github.com/SoftwareImpacts/SIMPAC-2021-29
#   6. SPARQL benchmarks (tests/sparql_benchmarks.rs)
#      Derived from: https://www.w3.org/wiki/SparqlBenchmarks (SP2B + BSBM)
#   7. sparqloscope functional tests (tests/sparqloscope_conformance.rs)
#      Derived from: https://github.com/ad-freiburg/sparqloscope
#
# Usage:
#   ./scripts/run_tests.sh [--unit] [--integration] [--w3c] [--geo] [--bench] [--scope] [--all]
#   Default: runs all tests
# ═══════════════════════════════════════════════════════════════════════════════

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

RUN_UNIT=false
RUN_INTEGRATION=false
RUN_W3C=false
RUN_RDF=false
RUN_GEO=false
RUN_BENCH=false
RUN_SCOPE=false
ALL=true

while [[ $# -gt 0 ]]; do
    case "$1" in
        --unit)        RUN_UNIT=true;        ALL=false; shift ;;
        --integration) RUN_INTEGRATION=true; ALL=false; shift ;;
        --w3c)         RUN_W3C=true;         ALL=false; shift ;;
        --rdf)         RUN_RDF=true;         ALL=false; shift ;;
        --geo)         RUN_GEO=true;         ALL=false; shift ;;
        --bench)       RUN_BENCH=true;       ALL=false; shift ;;
        --scope)       RUN_SCOPE=true;       ALL=false; shift ;;
        --all)         ALL=true;             shift ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ "$ALL" = true ]; then
    RUN_UNIT=true; RUN_INTEGRATION=true; RUN_W3C=true; RUN_RDF=true
    RUN_GEO=true; RUN_BENCH=true; RUN_SCOPE=true
fi

log()  { echo -e "${CYAN}[INFO]${NC} $*"; }
ok()   { echo -e "${GREEN}[PASS]${NC} $*"; }
fail() { echo -e "${RED}[FAIL]${NC} $*"; }

PASS=0
FAIL=0

run_suite() {
    local name="$1"
    local cmd="${2:-}"
    local opts="${3:---test-threads=4}"

    log "Running: $name"
    if eval "$cmd" -- $opts 2>&1 | tail -5; then
        ok "$name"
        PASS=$((PASS + 1))
    else
        fail "$name"
        FAIL=$((FAIL + 1))
    fi
    echo ""
}

echo -e "${BOLD}${CYAN}"
echo "╔══════════════════════════════════════════════════════════╗"
echo "║         Open Triplestore — Full Test Suite             ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo -e "${NC}"

# 1. Unit tests
if [ "$RUN_UNIT" = true ]; then
    echo -e "${BOLD}── 1. Unit Tests (src library) ──${NC}"
    run_suite "Unit tests" "cargo test --lib"
fi

# 2. Original integration tests
if [ "$RUN_INTEGRATION" = true ]; then
    echo -e "${BOLD}── 2. Integration Tests ──${NC}"
    run_suite "Integration tests" "cargo test --test integration_test"
fi

# 3. W3C SPARQL 1.1 conformance
if [ "$RUN_W3C" = true ]; then
    echo -e "${BOLD}── 3. W3C SPARQL 1.1 Conformance (from W3C test suite + sparql-conformance) ──${NC}"
    run_suite "W3C SPARQL 1.1 conformance" "cargo test --test w3c_sparql11_conformance"
fi

# 4. W3C RDF 1.1 conformance
if [ "$RUN_RDF" = true ]; then
    echo -e "${BOLD}── 4. W3C RDF 1.1 Conformance (from w3c/rdf-tests) ──${NC}"
    run_suite "W3C RDF 1.1 conformance" "cargo test --test rdf11_conformance"
fi

# 5. GeoSPARQL conformance
if [ "$RUN_GEO" = true ]; then
    echo -e "${BOLD}── 5. OGC GeoSPARQL 1.1 Conformance (from SIMPAC-2021-29) ──${NC}"
    run_suite "GeoSPARQL conformance" "cargo test --test geosparql_conformance"
fi

# 6. SPARQL benchmarks
if [ "$RUN_BENCH" = true ]; then
    echo -e "${BOLD}── 6. SPARQL Benchmarks (SP2B + Berlin/BSBM) ──${NC}"
    run_suite "SPARQL benchmarks" "cargo test --test sparql_benchmarks"
fi

# 7. sparqloscope functional tests
if [ "$RUN_SCOPE" = true ]; then
    echo -e "${BOLD}── 7. sparqloscope SPARQL Functional Coverage ──${NC}"
    run_suite "sparqloscope conformance" "cargo test --test sparqloscope_conformance"
fi

# ─── Summary ──────────────────────────────────────────────────────────────────
echo -e "${BOLD}${CYAN}═══ Test Suite Summary ═══${NC}"
echo ""
echo -e "Test suites passed: ${GREEN}${PASS}${NC}"
echo -e "Test suites failed: ${RED}${FAIL}${NC}"
echo ""

if [ "$FAIL" -eq 0 ]; then
    echo -e "${GREEN}${BOLD}✓ All test suites passed!${NC}"
    echo ""
    echo "To run the full W3C endpoint conformance suite against a live server:"
    echo "  cargo run --release -- --port 7878 &"
    echo "  ./scripts/run_w3c_conformance.sh"
    exit 0
else
    echo -e "${RED}${BOLD}✗ $FAIL suite(s) failed.${NC}"
    exit 1
fi
