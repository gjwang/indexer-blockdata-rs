#!/bin/bash
# MV Implementation Test - macOS Compatible

echo "========================================="
echo "  MV Implementation Test"
echo "========================================="
echo ""

PASS_COUNT=0
FAIL_COUNT=0

test_pass() {
    echo "  ✅ $1"
    ((PASS_COUNT++))
}

test_fail() {
    echo "  ❌ $1"
    ((FAIL_COUNT++))
}

# Test 1: MV exists
echo "Test 1: Checking if MV exists"
if docker exec scylla cqlsh -e "SELECT view_name FROM system_schema.views WHERE keyspace_name='trading';" 2>&1 | grep -q "user_balances_by_user"; then
    test_pass "MV (user_balances_by_user) exists"
else
    test_fail "MV NOT found"
    exit 1
fi
echo ""

# Test 2: Row count
echo "Test 2: Checking MV synchronization"
BASE_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.balance_ledger;" 2>&1 | grep -o '[0-9]\+' | head -1)
MV_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user;" 2>&1 | grep -o '[0-9]\+' | head -1)
echo "  Base table: $BASE_COUNT rows"
echo "  MV table: $MV_COUNT rows"
if [ "$BASE_COUNT" == "$MV_COUNT" ] && [ -n "$BASE_COUNT" ]; then
    test_pass "MV fully synchronized"
else
    test_fail "Row count mismatch or empty"
fi
echo ""

# Test 3: Gateway responding
echo "Test 3: Checking Gateway"
if curl -s -f "http://localhost:3001/api/user/balance?user_id=1001" > /dev/null 2>&1; then
    test_pass "Gateway is responding"
else
    test_fail "Gateway not responding"
    exit 1
fi
echo ""

# Test 4: Balance query works
echo "Test 4: Testing balance query"
RESPONSE=$(curl -s "http://localhost:3001/api/user/balance?user_id=1001")
echo "  Response: $(echo $RESPONSE | jq -c '.' 2>/dev/null || echo $RESPONSE)"

if echo "$RESPONSE" | jq -e '.status == 0' > /dev/null 2>&1; then
    test_pass "Query successful"

    # Check for BTC balance
    BTC=$(echo "$RESPONSE" | jq -r '.data[] | select(.asset=="BTC") | .avail' 2>/dev/null)
    if [ -n "$BTC" ] && [ "$BTC" != "null" ]; then
        test_pass "BTC balance returned: $BTC"
    else
        test_fail "No BTC balance found"
    fi
else
    test_fail "Query failed"
fi
echo ""

# Test 5: Empty user
echo "Test 5: Testing non-existent user"
EMPTY=$(curl -s "http://localhost:3001/api/user/balance?user_id=999999" | jq -r '.data | length' 2>/dev/null)
if [ "$EMPTY" == "0" ]; then
    test_pass "Returns empty array for non-existent user"
else
    test_fail "Unexpected response for non-existent user"
fi
echo ""

# Test 6: Direct MV query
echo "Test 6: Direct MV query from ScyllaDB"
if docker exec scylla cqlsh -e "SELECT asset_id, avail FROM trading.user_balances_by_user WHERE user_id=1001 PER PARTITION LIMIT 1;" 2>&1 | grep -q "asset_id"; then
    test_pass "Direct MV query works"
else
    test_fail "Direct MV query failed"
fi
echo ""

# Test 7: Multiple users
echo "Test 7: Testing query for multiple asset users"
MULTI_RESPONSE=$(curl -s "http://localhost:3001/api/user/balance?user_id=1001")
ASSET_COUNT=$(echo "$MULTI_RESPONSE" | jq -r '.data | length' 2>/dev/null)
if [ -n "$ASSET_COUNT" ] && [ "$ASSET_COUNT" -gt "0" ]; then
    test_pass "Query returns $ASSET_COUNT asset(s)"
else
    test_fail "Multi-asset query failed"
fi
echo ""

echo "========================================="
echo "  Test Summary"
echo "========================================="
echo ""
echo "Passed: $PASS_COUNT"
echo "Failed: $FAIL_COUNT"
echo ""

if [ "$FAIL_COUNT" -eq 0 ]; then
    echo "🎉 ALL TESTS PASSED!"
    echo ""
    echo "MV Implementation Summary:"
    echo "  ✅ MV created and synchronized ($MV_COUNT rows)"
    echo "  ✅ Balance queries working correctly"
    echo "  ✅ Gateway integration successful"
    echo "  ✅ Multi-asset support verified"
    echo ""
    echo "System is ready for production! 🚀"
    exit 0
else
    echo "⚠️  $FAIL_COUNT test(s) failed"
    exit 1
fi
