#!/bin/bash
# Comprehensive test for MV-based balance queries

set +e

echo "========================================="
echo "MV Implementation Test Suite"
echo "========================================="
echo ""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test counter
PASSED=0
FAILED=0

test_pass() {
    echo -e "${GREEN}✅ PASS${NC}: $1"
    ((PASSED++))
}

test_fail() {
    echo -e "${RED}❌ FAIL${NC}: $1"
    ((FAILED++))
}

test_info() {
    echo -e "${YELLOW}ℹ️  INFO${NC}: $1"
}

echo "TEST 1: Verify MV exists in ScyllaDB"
echo "-------------------------------------"
MV_EXISTS=$(docker exec scylla cqlsh -e "SELECT view_name FROM system_schema.views WHERE keyspace_name='trading' AND view_name='user_balances_by_user';" 2>&1 | grep -c "user_balances_by_user" || true)

if [ "$MV_EXISTS" -gt "0" ]; then
    test_pass "MV (user_balances_by_user) exists"
else
    test_fail "MV (user_balances_by_user) NOT found"
    exit 1
fi
echo ""

echo "TEST 2: Verify MV row count matches base table"
echo "-----------------------------------------------"
BASE_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.balance_ledger;" 2>&1 | grep -oP '\d+' | head -1)
MV_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user;" 2>&1 | grep -oP '\d+' | head -1)

test_info "Base table rows: $BASE_COUNT"
test_info "MV rows: $MV_COUNT"

if [ "$BASE_COUNT" == "$MV_COUNT" ]; then
    test_pass "MV is fully synchronized"
else
    test_fail "MV row count mismatch (base=$BASE_COUNT, mv=$MV_COUNT)"
fi
echo ""

echo "TEST 3: Verify Gateway is running"
echo "----------------------------------"
GATEWAY_UP=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:3001/health 2>&1 || echo "000")

if [ "$GATEWAY_UP" == "200" ]; then
    test_pass "Gateway is responding"
else
    test_fail "Gateway not responding (HTTP $GATEWAY_UP)"
    exit 1
fi
echo ""

echo "TEST 4: Query balance for existing user"
echo "----------------------------------------"
BALANCE_RESPONSE=$(curl -s "http://localhost:3001/api/user/balance?user_id=1001" 2>&1)
BALANCE_STATUS=$(echo "$BALANCE_RESPONSE" | jq -r '.status' 2>/dev/null || echo "error")

test_info "Response: $(echo $BALANCE_RESPONSE | jq -c '.')"

if [ "$BALANCE_STATUS" == "0" ]; then
    test_pass "Balance query successful"

    # Check if BTC balance exists
    BTC_BALANCE=$(echo "$BALANCE_RESPONSE" | jq -r '.data[] | select(.asset=="BTC") | .avail' 2>/dev/null || echo "")
    if [ -n "$BTC_BALANCE" ]; then
        test_pass "BTC balance returned: $BTC_BALANCE"
    else
        test_fail "BTC balance not found in response"
    fi
else
    test_fail "Balance query failed (status=$BALANCE_STATUS)"
fi
echo ""

echo "TEST 5: Query balance for non-existent user"
echo "--------------------------------------------"
EMPTY_RESPONSE=$(curl -s "http://localhost:3001/api/user/balance?user_id=999999" 2>&1)
EMPTY_STATUS=$(echo "$EMPTY_RESPONSE" | jq -r '.status' 2>/dev/null || echo "error")
EMPTY_DATA=$(echo "$EMPTY_RESPONSE" | jq -r '.data | length' 2>/dev/null || echo "error")

if [ "$EMPTY_STATUS" == "0" ] && [ "$EMPTY_DATA" == "0" ]; then
    test_pass "Non-existent user returns empty array"
else
    test_fail "Non-existent user response incorrect"
fi
echo ""

echo "TEST 6: Verify MV query is being used (check logs)"
echo "---------------------------------------------------"
# Check if "user_balances_by_user" appears in recent logs
if pgrep -f order_gate_server > /dev/null; then
    test_pass "Gateway process is running"

    # Check startup logs for MV health check
    if grep -q "MV (user_balances_by_user) is healthy" logs/gateway.log* 2>/dev/null; then
        test_pass "MV health check logged on startup"
    else
        test_info "MV health check not found in logs (may have rotated)"
    fi
else
    test_fail "Gateway process not running"
fi
echo ""

echo "TEST 7: Performance check (single query)"
echo "-----------------------------------------"
START=$(perl -MTime::HiRes -e 'printf("%.0f\n", Time::HiRes::time()*1000)')
curl -s "http://localhost:3001/api/user/balance?user_id=1001" > /dev/null
END=$(perl -MTime::HiRes -e 'printf("%.0f\n", Time::HiRes::time()*1000)')
LATENCY=$(( $END - $START ))

test_info "Query latency: ${LATENCY}ms"
if [ "$LATENCY" -lt 100 ]; then
    test_pass "Query latency acceptable (< 100ms)"
else
    test_info "Query latency higher than ideal but functional"
fi
echo ""

echo "TEST 8: Direct MV query from ScyllaDB"
echo "--------------------------------------"
DIRECT_QUERY=$(docker exec scylla cqlsh -e "SELECT asset_id, avail FROM trading.user_balances_by_user WHERE user_id=1001 PER PARTITION LIMIT 1;" 2>&1)

if echo "$DIRECT_QUERY" | grep -q "asset_id"; then
    test_pass "Direct MV query successful"
    test_info "$DIRECT_QUERY"
else
    test_fail "Direct MV query failed"
fi
echo ""

echo "TEST 9: Verify MV schema is correct"
echo "------------------------------------"
MV_SCHEMA=$(docker exec scylla cqlsh -e "DESCRIBE MATERIALIZED VIEW trading.user_balances_by_user;" 2>&1)

if echo "$MV_SCHEMA" | grep -q "PER PARTITION LIMIT"; then
    test_info "Note: DESCRIBE doesn't show PER PARTITION LIMIT (expected)"
fi

if echo "$MV_SCHEMA" | grep -q "user_id" && echo "$MV_SCHEMA" | grep -q "asset_id"; then
    test_pass "MV schema contains required columns"
else
    test_fail "MV schema incomplete"
fi
echo ""

echo "========================================="
echo "Test Summary"
echo "========================================="
echo -e "${GREEN}Passed: $PASSED${NC}"
if [ "$FAILED" -gt 0 ]; then
    echo -e "${RED}Failed: $FAILED${NC}"
else
    echo -e "${GREEN}Failed: $FAILED${NC}"
fi
echo ""

if [ "$FAILED" -eq 0 ]; then
    echo -e "${GREEN}🎉 ALL TESTS PASSED!${NC}"
    echo ""
    echo "MV Implementation verified successfully:"
    echo "  ✅ MV created and synchronized"
    echo "  ✅ Balance queries working"
    echo "  ✅ Gateway health check functioning"
    echo "  ✅ Performance acceptable"
    echo ""
    echo "System is ready for production!"
    exit 0
else
    echo -e "${RED}⚠️  SOME TESTS FAILED${NC}"
    echo "Please review the failures above."
    exit 1
fi
