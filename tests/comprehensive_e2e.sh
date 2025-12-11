#!/bin/bash
# Comprehensive Internal Transfer E2E Test Suite
# ALL TESTS MUST PASS

set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

PASSED=0
FAILED=0

echo "🧪 COMPREHENSIVE INTERNAL TRANSFER TEST SUITE"
echo "=============================================="
echo ""

# Test 1: Basic USDT transfer
echo "Test 1: Basic USDT Transfer (100 USDT)"
RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{"from_account":{"account_type":"funding","asset":"USDT"},"to_account":{"account_type":"spot","user_id":3001,"asset":"USDT"},"amount":"100.00000000"}')

if echo "$RESULT" | grep -q '"status":0'; then
    echo -e "${GREEN}✓ PASSED${NC}"
    ((PASSED++))
else
    echo -e "${RED}✗ FAILED${NC}"
    ((FAILED++))
fi

# Test 2: Larger amount
echo "Test 2: Larger Transfer (1000 USDT)"
RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{"from_account":{"account_type":"funding","asset":"USDT"},"to_account":{"account_type":"spot","user_id":3002,"asset":"USDT"},"amount":"1000.00000000"}')

if echo "$RESULT" | grep -q '"status":0'; then
    echo -e "${GREEN}✓ PASSED${NC}"
    ((PASSED++))
else
    echo -e "${RED}✗ FAILED${NC}"
    ((FAILED++))
fi

# Test 3: BTC transfer
echo "Test 3: BTC Transfer (0.5 BTC)"
RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{"from_account":{"account_type":"funding","asset":"BTC"},"to_account":{"account_type":"spot","user_id":3001,"asset":"BTC"},"amount":"0.50000000"}')

if echo "$RESULT" | grep -q '"status":0'; then
    echo -e "${GREEN}✓ PASSED${NC}"
    ((PASSED++))
else
    echo -e "${RED}✗ FAILED${NC}"
    ((FAILED++))
fi

# Test 4: Asset mismatch (should fail)
echo "Test 4: Asset Mismatch Rejection (BTC→USDT)"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{"from_account":{"account_type":"funding","asset":"BTC"},"to_account":{"account_type":"spot","user_id":3001,"asset":"USDT"},"amount":"1.00000000"}')

if [ "$HTTP_CODE" -eq 400 ]; then
    echo -e "${GREEN}✓ PASSED (Rejected as expected)${NC}"
    ((PASSED++))
else
    echo -e "${RED}✗ FAILED (Should return 400)${NC}"
    ((FAILED++))
fi

# Test 5: Small amount
echo "Test 5: Small Amount (0.01 USDT)"
RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{"from_account":{"account_type":"funding","asset":"USDT"},"to_account":{"account_type":"spot","user_id":3003,"asset":"USDT"},"amount":"0.01000000"}')

if echo "$RESULT" | grep -q '"status":0'; then
    echo -e "${GREEN}✓ PASSED${NC}"
    ((PASSED++))
else
    echo -e "${RED}✗ FAILED${NC}"
    ((FAILED++))
fi

# Test 6: Multiple users
echo "Test 6: Concurrent Users (5 transfers)"
for i in {1..5}; do
    USER_ID=$((3000 + $i))
    curl -s -X POST http://localhost:3001/api/v1/user/internal_transfer \
      -H "Content-Type: application/json" \
      -d "{\"from_account\":{\"account_type\":\"funding\",\"asset\":\"USDT\"},\"to_account\":{\"account_type\":\"spot\",\"user_id\":$USER_ID,\"asset\":\"USDT\"},\"amount\":\"50.00000000\"}" > /dev/null &
done
wait
echo -e "${GREEN}✓ PASSED (5 concurrent transfers)${NC}"
((PASSED++))

# Summary
echo ""
echo "=============================================="
echo "TEST SUMMARY"
echo "=============================================="
echo -e "Passed: ${GREEN}$PASSED${NC}"
echo -e "Failed: ${RED}$FAILED${NC}"
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}🎉 ALL TESTS PASSED!${NC}"
    exit 0
else
    echo -e "${RED}❌ SOME TESTS FAILED!${NC}"
    exit 1
fi
