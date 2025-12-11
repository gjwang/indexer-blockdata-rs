#!/bin/bash
# Test Suite Runner - Runs all tests in sequence

set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "================================="
echo "🧪 HFT Exchange Test Suite"
echo "================================="
echo ""

# Track results
PASSED=0
FAILED=0
TOTAL=0

# Test list
TESTS=(
    "01_infrastructure.sh:Infrastructure Validation"
    "02_ubscore_startup.sh:UBSCore Service Startup"
    "03_deposit_withdrawal.sh:Deposit & Withdrawal API"
    "05_gateway_e2e.sh:Gateway E2E (deposit + balance)"
)

run_test() {
    local test_file=$1
    local test_name=$2

    TOTAL=$((TOTAL + 1))

    echo ""
    echo "----------------------------------------"
    echo "Running: $test_name"
    echo "----------------------------------------"

    if bash "tests/$test_file"; then
        echo -e "${GREEN}✅ PASSED${NC}: $test_name"
        PASSED=$((PASSED + 1))
        return 0
    else
        echo -e "${RED}❌ FAILED${NC}: $test_name"
        FAILED=$((FAILED + 1))
        return 1
    fi
}

# Run all tests
for test_entry in "${TESTS[@]}"; do
    IFS=':' read -r test_file test_name <<< "$test_entry"
    run_test "$test_file" "$test_name" || true
done

# Summary
echo ""
echo "================================="
echo "📊 Test Summary"
echo "================================="
echo "Total:  $TOTAL"
echo -e "${GREEN}Passed: $PASSED${NC}"
if [ $FAILED -gt 0 ]; then
    echo -e "${RED}Failed: $FAILED${NC}"
fi
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}🎉 All tests passed!${NC}"
    exit 0
else
    echo -e "${RED}❌ Some tests failed${NC}"
    exit 1
fi
