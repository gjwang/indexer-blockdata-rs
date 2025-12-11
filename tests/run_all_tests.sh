#!/bin/bash
set -e

# Full Test Suite Runner
# Runs all tests from 01 to 08 sequentially

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  🧪 HFT EXCHANGE - FULL TEST SUITE"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Running all tests from 01 to 08..."
echo "Press Ctrl+C to abort"
echo ""
sleep 2

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Track results
TESTS_PASSED=0
TESTS_FAILED=0
START_TIME=$(date +%s)

# Function to run a test
run_test() {
    local test_num=$1
    local test_name=$2
    local test_script=$3

    echo ""
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${YELLOW}TEST $test_num: $test_name${NC}"
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

    if bash "$test_script"; then
        echo -e "${GREEN}✅ TEST $test_num PASSED${NC}"
        TESTS_PASSED=$((TESTS_PASSED + 1))
        return 0
    else
        echo -e "${RED}❌ TEST $test_num FAILED${NC}"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        return 1
    fi
}

# Clean environment first
echo "🧹 Cleaning environment..."
pkill -f "order_gate_server|ubscore_aeron|matching_engine|settlement_service" 2>/dev/null || true
docker-compose down -v 2>/dev/null || true
sleep 2

# Run tests sequentially
run_test "01" "Infrastructure Setup" "tests/01_infrastructure.sh" || exit 1
run_test "02" "UBSCore Startup" "tests/02_ubscore_startup.sh" || exit 1

# Test 03 starts services - they remain running
run_test "03" "Start Services" "tests/03_start_services.sh" || exit 1

# Test 04 uses services from test 03
run_test "04" "HTTP API Tests" "tests/04_http_api_test.sh" || exit 1

# Test 05 is standalone (has its own setup/cleanup)
echo ""
echo -e "${YELLOW}⏭️  Skipping Test 05 (standalone E2E) - comprehensive test${NC}"

# Test 06 needs services + ME
run_test "06" "Matching Engine" "tests/06_matching_engine.sh" || exit 1

# Test 07 needs all services including Settlement
run_test "07" "Settlement Service" "tests/07_settlement.sh" || exit 1

# Test 08 needs all services for full trading flow
run_test "08" "Full Trading E2E" "tests/08_full_trading_e2e.sh" || exit 1

# Calculate duration
END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

# Final summary
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  📊 FINAL RESULTS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo -e "  ${GREEN}✅ Tests Passed: $TESTS_PASSED${NC}"
echo -e "  ${RED}❌ Tests Failed: $TESTS_FAILED${NC}"
echo "  ⏱️  Total Duration: ${DURATION}s"
echo ""

if [ $TESTS_FAILED -eq 0 ]; then
    echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}  🎉 ALL TESTS PASSED! 🎉${NC}"
    echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo "💡 Services are still running. To stop:"
    echo "   pkill -f 'order_gate_server|ubscore_aeron_service|matching_engine_server|settlement_service'"
    echo ""
    exit 0
else
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${RED}  ❌ SOME TESTS FAILED ❌${NC}"
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    exit 1
fi
