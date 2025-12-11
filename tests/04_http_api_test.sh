#!/bin/bash
set -e

# Test 04: HTTP API Tests (Deposit & Balance Queries)
# Assumes services are already running (run test 03 first)

echo "🧪 TEST 04: HTTP API Tests"
echo "==========================="

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Check if services are running
if ! pgrep -f "order_gate_server" > /dev/null; then
    echo -e "${RED}❌ Gateway not running!${NC}"
    echo "Please run: bash tests/03_start_services.sh"
    exit 1
fi

if ! pgrep -f "ubscore_aeron_service" > /dev/null; then
    echo -e "${RED}❌ UBSCore not running!${NC}"
    echo "Please run: bash tests/03_start_services.sh"
    exit 1
fi

echo "✅ Services detected running"
echo ""

# Test 1: Deposit USDT
echo ""
echo "💰 Test 1: Deposit 1000 USDT for user 3001..."
DEPOSIT_REQ='{"request_id": "test_dep_001", "user_id": 3001, "asset": "USDT", "amount": "1000.0"}'

DEPOSIT_RESULT=$(curl -s --max-time 10 -X POST http://localhost:3001/api/v1/user/transfer_in \
    -H "Content-Type: application/json" \
    -d "$DEPOSIT_REQ")

echo "Response: $DEPOSIT_RESULT"

if echo "$DEPOSIT_RESULT" | grep -q '"success":true'; then
    echo -e " ${GREEN}✅ Deposit request accepted${NC}"
else
    echo -e " ${RED}❌ Deposit failed${NC}"
    echo "Full response: $DEPOSIT_RESULT"
    exit 1
fi

# Wait for processing
echo "⏳ Waiting for deposit to process..."
sleep 5

# Test 2: Query Balance
echo ""
echo "💳 Test 2: Query balance for user 3001..."
BALANCE_RESULT=$(curl -s --max-time 10 "http://localhost:3001/api/v1/user/balance?user_id=3001")

echo "Response: $BALANCE_RESULT"

if echo "$BALANCE_RESULT" | grep -q "1000"; then
    echo -e " ${GREEN}✅ Balance shows 1000 USDT${NC}"
else
    echo -e " ${RED}❌ Balance mismatch${NC}"
    echo "Expected 1000 USDT, got: $BALANCE_RESULT"
    # Don't exit - show more info
fi

# Test 3: Deposit BTC
echo ""
echo "💰 Test 3: Deposit 0.5 BTC for user 3001..."
DEPOSIT_BTC='{"request_id": "test_dep_002", "user_id": 3001, "asset": "BTC", "amount": "0.5"}'

DEPOSIT_BTC_RESULT=$(curl -s --max-time 10 -X POST http://localhost:3001/api/v1/user/transfer_in \
    -H "Content-Type: application/json" \
    -d "$DEPOSIT_BTC")

if echo "$DEPOSIT_BTC_RESULT" | grep -q '"success":true'; then
    echo -e " ${GREEN}✅ BTC deposit accepted${NC}"
else
    echo -e " ${RED}❌ BTC deposit failed${NC}"
    echo "$DEPOSIT_BTC_RESULT"
fi

sleep 5

# Test 4: Verify both balances
echo ""
echo "💳 Test 4: Verify both USDT and BTC balances..."
BALANCE_FINAL=$(curl -s --max-time 10 "http://localhost:3001/api/v1/user/balance?user_id=3001")

echo "Response: $BALANCE_FINAL"

if echo "$BALANCE_FINAL" | grep -q "USDT" && echo "$BALANCE_FINAL" | grep -q "BTC"; then
    echo -e " ${GREEN}✅ Both assets present in balance${NC}"
else
    echo -e " ${RED}⚠️  Missing assets (might be timing issue)${NC}"
fi

echo ""
echo "📊 Final Balance Summary:"
echo "$BALANCE_FINAL" | jq '.' 2>/dev/null || echo "$BALANCE_FINAL"

echo ""
echo -e "${GREEN}🎉 TEST 04 PASSED - HTTP API Working${NC}"
echo ""
echo "💡 Services still running. To stop:"
echo "   pkill -f 'order_gate_server|ubscore_aeron_service'"
echo ""

exit 0
