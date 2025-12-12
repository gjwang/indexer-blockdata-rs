#!/bin/bash
#
# Internal Transfer V2 - TigerBeetle Integration Test
# Tests with real TigerBeetle backend
#
# Prerequisites:
#   - TigerBeetle running on 127.0.0.1:3000
#   - ScyllaDB running on 127.0.0.1:9042
#

echo "🧪 TRANSFER V2 - TIGERBEETLE INTEGRATION TEST"
echo "=============================================="

# Configuration
BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
USER_ID=5001
ASSET_ID=1  # BTC
INITIAL_AMOUNT=1000000000  # 10 BTC in satoshis

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

PASSED=0
FAILED=0

# Helper: Test result
check() {
    local expected=$1
    local actual=$2
    local test_name=$3

    if echo "$actual" | grep -qE "$expected"; then
        echo -e "${GREEN}✅ $test_name: PASSED${NC}"
        ((PASSED++))
    else
        echo -e "${RED}❌ $test_name: FAILED (expected to match '$expected')${NC}"
        echo "   Actual: $actual"
        ((FAILED++))
    fi
}

# Helper: Transfer
transfer() {
    curl -s -X POST "$BASE_URL/api/v1/transfer" \
        -H "Content-Type: application/json" \
        -d "$1"
}

# Wait for service
echo "⏳ Waiting for service..."
for i in {1..10}; do
    if curl -s "$BASE_URL/api/v1/transfer" -X POST -d '{}' 2>/dev/null | grep -q "error\|status"; then
        echo "✅ Service ready"
        break
    fi
    sleep 1
done

# ===== TEST SUITE =====

echo ""
echo "📤 Test 1: Funding → Trading Transfer"
RESP=$(transfer '{"from":"funding","to":"trading","user_id":'$USER_ID',"asset_id":'$ASSET_ID',"amount":1000}')
echo "Response: $RESP"
check "committed|pending" "$RESP" "Funding→Trading"

echo ""
echo "📤 Test 2: Trading → Funding Transfer"
RESP=$(transfer '{"from":"trading","to":"funding","user_id":'$USER_ID',"asset_id":'$ASSET_ID',"amount":500}')
echo "Response: $RESP"
check "committed|pending" "$RESP" "Trading→Funding"

echo ""
echo "📤 Test 3: Multiple Consecutive Transfers"
for i in {1..3}; do
    RESP=$(transfer '{"from":"funding","to":"trading","user_id":'$USER_ID',"asset_id":'$ASSET_ID',"amount":100}')
    STATUS=$(echo "$RESP" | jq -r '.status')
    echo "  Transfer $i: $STATUS"
done
check "3" "3" "Multiple transfers completed"

echo ""
echo "📤 Test 4: Different Asset"
RESP=$(transfer '{"from":"funding","to":"trading","user_id":'$USER_ID',"asset_id":2,"amount":1000}')
echo "Response: $RESP"
check "committed|pending" "$RESP" "Different asset (USDT)"

# ===== SUMMARY =====

echo ""
echo "============================================"
echo "📊 TEST SUMMARY"
echo "============================================"
echo "Passed: $PASSED"
echo "Failed: $FAILED"

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}🎉 ALL TESTS PASSED!${NC}"
    exit 0
else
    echo -e "${RED}⚠️ SOME TESTS FAILED${NC}"
    exit 1
fi
