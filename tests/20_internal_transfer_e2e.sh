#!/bin/bash
#
# Internal Transfer End-to-End Test
# Tests the FSM-based internal transfer system
#
# set -e removed to allow all tests to run

echo "🧪 INTERNAL TRANSFER E2E TEST"
echo "=============================="

# Configuration
BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
TIMEOUT=30

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Test counters
PASSED=0
FAILED=0

# Helper: Make transfer request
transfer() {
    local from=$1
    local to=$2
    local user_id=$3
    local asset_id=$4
    local amount=$5

    curl -s -X POST "$BASE_URL/api/v1/transfer" \
        -H "Content-Type: application/json" \
        -d "{\"from\":\"$from\",\"to\":\"$to\",\"user_id\":$user_id,\"asset_id\":$asset_id,\"amount\":$amount}"
}

# Helper: Query transfer
query() {
    local req_id=$1
    curl -s "$BASE_URL/api/v1/transfer/$req_id"
}

# Helper: Assert status
assert_status() {
    local expected=$1
    local actual=$2
    local test_name=$3

    if [ "$actual" == "$expected" ]; then
        echo -e "${GREEN}✅ $test_name: PASSED${NC}"
        ((PASSED++))
    else
        echo -e "${RED}❌ $test_name: FAILED (expected $expected, got $actual)${NC}"
        ((FAILED++))
    fi
}

# Helper: Assert contains
assert_contains() {
    local needle=$1
    local haystack=$2
    local test_name=$3

    if [[ "$haystack" == *"$needle"* ]]; then
        echo -e "${GREEN}✅ $test_name: PASSED${NC}"
        ((PASSED++))
    else
        echo -e "${RED}❌ $test_name: FAILED (expected to contain '$needle')${NC}"
        ((FAILED++))
    fi
}

# Wait for service
echo "⏳ Waiting for gateway..."
for i in {1..30}; do
    if curl -s "$BASE_URL/api/v1/transfer" -X POST -d '{}' 2>/dev/null | grep -q "error\|status"; then
        echo "✅ Gateway is ready"
        break
    fi
    sleep 1
done

# ===== VALIDATION TESTS =====

echo ""
echo "📤 Test Suite 1: Input Validation"
echo "-----------------------------------"

# Test 1.1: Zero amount
echo "Test 1.1: Zero amount"
RESP=$(transfer "funding" "trading" 4001 1 0)
echo "Response: $RESP"
ERROR=$(echo "$RESP" | jq -r '.error // .status')
assert_contains "0" "$ERROR" "Zero amount rejected"

# Test 1.2: Invalid source
echo "Test 1.2: Invalid source"
RESP=$(transfer "invalid" "trading" 4001 1 1000)
echo "Response: $RESP"
STATUS=$(echo "$RESP" | jq -r '.status')
assert_status "failed" "$STATUS" "Invalid source rejected"

# Test 1.3: Same source and target
echo "Test 1.3: Same source and target"
RESP=$(transfer "funding" "funding" 4001 1 1000)
echo "Response: $RESP"
STATUS=$(echo "$RESP" | jq -r '.status')
assert_status "failed" "$STATUS" "Same source/target rejected"

# ===== HAPPY PATH TESTS =====

echo ""
echo "📤 Test Suite 2: Happy Path"
echo "----------------------------"

# Test 2.1: Funding → Trading
echo "Test 2.1: Funding → Trading"
RESP=$(transfer "funding" "trading" 4001 1 1000000)
echo "Response: $RESP"
STATUS=$(echo "$RESP" | jq -r '.status')
REQ_ID=$(echo "$RESP" | jq -r '.req_id')
echo "req_id: $REQ_ID, status: $STATUS"

if [ "$STATUS" == "committed" ] || [ "$STATUS" == "pending" ]; then
    echo -e "${GREEN}✅ Funding → Trading: PASSED ($STATUS)${NC}"
    ((PASSED++))
else
    echo -e "${RED}❌ Funding → Trading: FAILED (status: $STATUS)${NC}"
    ((FAILED++))
fi

# Test 2.2: Trading → Funding
echo "Test 2.2: Trading → Funding"
RESP=$(transfer "trading" "funding" 4001 1 500000)
echo "Response: $RESP"
STATUS=$(echo "$RESP" | jq -r '.status')

if [ "$STATUS" == "committed" ] || [ "$STATUS" == "pending" ]; then
    echo -e "${GREEN}✅ Trading → Funding: PASSED ($STATUS)${NC}"
    ((PASSED++))
else
    echo -e "${RED}❌ Trading → Funding: FAILED (status: $STATUS)${NC}"
    ((FAILED++))
fi

# Test 2.3: Query transfer status
echo "Test 2.3: Query transfer status"
if [ -n "$REQ_ID" ] && [ "$REQ_ID" != "null" ]; then
    RESP=$(query "$REQ_ID")
    echo "Response: $RESP"
    STATE=$(echo "$RESP" | jq -r '.state')
    echo "State: $STATE"

    if [ -n "$STATE" ] && [ "$STATE" != "null" ]; then
        echo -e "${GREEN}✅ Query transfer: PASSED (state: $STATE)${NC}"
        ((PASSED++))
    else
        echo -e "${RED}❌ Query transfer: FAILED${NC}"
        ((FAILED++))
    fi
else
    echo -e "${YELLOW}⚠️ Skipping query test (no req_id)${NC}"
fi

# ===== ERROR HANDLING TESTS =====

echo ""
echo "📤 Test Suite 3: Error Handling"
echo "---------------------------------"

# Test 3.1: Query non-existent transfer (use valid decimal ID that doesn't exist)
echo "Test 3.1: Query non-existent"
RESP=$(query "9999999999999999")
echo "Response: $RESP"
ERROR=$(echo "$RESP" | jq -r '.error')
assert_contains "not found" "$ERROR" "Query non-existent"

# Test 3.2: Invalid req_id format
echo "Test 3.2: Invalid req_id format"
RESP=$(query "not-a-uuid")
echo "Response: $RESP"
ERROR=$(echo "$RESP" | jq -r '.error')
assert_contains "Invalid" "$ERROR" "Invalid UUID format"

# ===== SUMMARY =====

echo ""
echo "================================"
echo "📊 TEST SUMMARY"
echo "================================"
echo "Passed: $PASSED"
echo "Failed: $FAILED"

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}🎉 ALL TESTS PASSED!${NC}"
    exit 0
else
    echo -e "${RED}❌ SOME TESTS FAILED${NC}"
    exit 1
fi
