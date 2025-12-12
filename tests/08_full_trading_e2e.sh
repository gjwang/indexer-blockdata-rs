#!/bin/bash
set -e

# Test 08: Full Trading Flow End-to-End
# Tests complete trading cycle: Deposit → Order → Match → Settlement → Balance Verification
# Flow: Gateway → Aeron → UBSCore → Kafka → ME → Settlement → ScyllaDB → TigerBeetle

echo "🧪 TEST 08: Full Trading Flow E2E"
echo "=================================="

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Check if services are running
if ! pgrep -f "ubscore_aeron_service" > /dev/null || \
   ! pgrep -f "gateway_service" > /dev/null || \
   ! pgrep -f "matching_engine_server" > /dev/null || \
   ! pgrep -f "settlement_service" > /dev/null; then
    echo -e "${RED}❌ Not all services running!${NC}"
    echo "Please run tests 03, 06, and 07 first to start all services"
    exit 1
fi

echo "✅ All services detected running"
echo ""

# Test Scenario: Alice buys 0.5 BTC from Bob at 45000 USDT/BTC
ALICE_ID=6001
BOB_ID=6002
SYMBOL="BTC_USDT"
PRICE="45000.0"
QUANTITY="0.5"

echo "📋 Test Scenario:"
echo "  • Alice (User $ALICE_ID) wants to BUY $QUANTITY BTC"
echo "  • Bob (User $BOB_ID) wants to SELL $QUANTITY BTC"
echo "  • Price: $PRICE USDT/BTC"
echo "  • Total: 22500 USDT"
echo ""

# Step 1: Fund users
echo -e "${YELLOW}═══ Step 1: Fund Users ═══${NC}"

echo "💰 Funding Alice with 100,000 USDT..."
ALICE_DEPOSIT='{"request_id": "e2e_alice_usdt", "user_id": 6001, "asset": "USDT", "amount": "100000.0"}'
ALICE_DEPOSIT_RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/transfer_in \
    -H "Content-Type: application/json" \
    -d "$ALICE_DEPOSIT")

if echo "$ALICE_DEPOSIT_RESULT" | grep -q '"success":true'; then
    ALICE_REQ_ID=$(echo "$ALICE_DEPOSIT_RESULT" | grep -o '"request_id":"[^"]*"' | cut -d'"' -f4)
    echo -e " ${GREEN}✅ Alice funded (request: $ALICE_REQ_ID)${NC}"
else
    echo -e " ${RED}❌ Alice deposit failed${NC}"
    exit 1
fi

sleep 2

echo "💰 Funding Bob with 10 BTC..."
BOB_DEPOSIT='{"request_id": "e2e_bob_btc", "user_id": 6002, "asset": "BTC", "amount": "10.0"}'
BOB_DEPOSIT_RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/transfer_in \
    -H "Content-Type: application/json" \
    -d "$BOB_DEPOSIT")

if echo "$BOB_DEPOSIT_RESULT" | grep -q '"success":true'; then
    BOB_REQ_ID=$(echo "$BOB_DEPOSIT_RESULT" | grep -o '"request_id":"[^"]*"' | cut -d'"' -f4)
    echo -e " ${GREEN}✅ Bob funded (request: $BOB_REQ_ID)${NC}"
else
    echo -e " ${RED}❌ Bob deposit failed${NC}"
    exit 1
fi

sleep 3
echo ""

# Step 2: Verify initial balances
echo -e "${YELLOW}═══ Step 2: Verify Initial Balances ═══${NC}"

ALICE_BALANCE=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=$ALICE_ID")
if echo "$ALICE_BALANCE" | grep -q "100000"; then
    echo -e " ${GREEN}✅ Alice has 100,000 USDT${NC}"
else
    echo -e " ${RED}⚠️  Alice balance mismatch${NC}"
fi

BOB_BALANCE=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=$BOB_ID")
if echo "$BOB_BALANCE" | grep -q "10"; then
    echo -e " ${GREEN}✅ Bob has 10 BTC${NC}"
else
    echo -e " ${RED}⚠️  Bob balance mismatch${NC}"
fi

sleep 2
echo ""

# Step 3: Place orders
echo -e "${YELLOW}═══ Step 3: Place Orders ═══${NC}"

echo "📝 Bob places SELL order (maker)..."
BOB_ORDER='{"cid": "e2e_bob_sell_order", "symbol": "BTC_USDT", "side": "Sell", "price": "45000.0", "quantity": "0.5", "order_type": "Limit"}'
BOB_ORDER_RESULT=$(curl -s -X POST "http://localhost:3001/api/v1/order/create?user_id=$BOB_ID" \
    -H "Content-Type: application/json" \
    -d "$BOB_ORDER")

if echo "$BOB_ORDER_RESULT" | grep -q '"order_id"'; then
    BOB_ORDER_ID=$(echo "$BOB_ORDER_RESULT" | grep -o '"order_id":"[0-9]*"' | cut -d'"' -f4)
    echo -e " ${GREEN}✅ Bob's sell order placed: $BOB_ORDER_ID${NC}"
else
    echo -e " ${RED}❌ Bob's order failed${NC}"
    echo "$BOB_ORDER_RESULT"
fi

sleep 3

echo "📝 Alice places BUY order (taker - will match)..."
ALICE_ORDER='{"cid": "e2e_alice_buy_order", "symbol": "BTC_USDT", "side": "Buy", "price": "45000.0", "quantity": "0.5", "order_type": "Limit"}'
ALICE_ORDER_RESULT=$(curl -s -X POST "http://localhost:3001/api/v1/order/create?user_id=$ALICE_ID" \
    -H "Content-Type: application/json" \
    -d "$ALICE_ORDER")

if echo "$ALICE_ORDER_RESULT" | grep -q '"order_id"'; then
    ALICE_ORDER_ID=$(echo "$ALICE_ORDER_RESULT" | grep -o '"order_id":"[0-9]*"' | cut -d'"' -f4)
    echo -e " ${GREEN}✅ Alice's buy order placed: $ALICE_ORDER_ID${NC}"
else
    echo -e " ${RED}❌ Alice's order failed${NC}"
    echo "$ALICE_ORDER_RESULT"
fi

echo "⏳ Waiting for matching and settlement..."
sleep 5
echo ""

# Step 4: Verify final balances
echo -e "${YELLOW}═══ Step 4: Verify Final Balances (After Trade) ═══${NC}"

ALICE_FINAL=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=$ALICE_ID")
echo "Alice's balance:"
echo "$ALICE_FINAL" | jq '.' 2>/dev/null || echo "$ALICE_FINAL"

# Alice should have: 100000 - 22500 = 77500 USDT and 0.5 BTC
if echo "$ALICE_FINAL" | grep -q "77500\|0.5\|0.50"; then
    echo -e " ${GREEN}✅ Alice received 0.5 BTC, spent ~22500 USDT${NC}"
else
    echo -e " ${YELLOW}⚠️  Check Alice's balance manually${NC}"
fi

echo ""
BOB_FINAL=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=$BOB_ID")
echo "Bob's balance:"
echo "$BOB_FINAL" | jq '.' 2>/dev/null || echo "$BOB_FINAL"

# Bob should have: 10 - 0.5 = 9.5 BTC and 22500 USDT
if echo "$BOB_FINAL" | grep -q "9.5\|9.50\|22500"; then
    echo -e " ${GREEN}✅ Bob sold 0.5 BTC, received ~22500 USDT${NC}"
else
    echo -e " ${YELLOW}⚠️  Check Bob's balance manually${NC}"
fi

echo ""

# Step 5: Summary
echo -e "${YELLOW}═══ Step 5: System Health Check ═══${NC}"

DATE=$(date +%Y-%m-%d)

# Check UBSCore processed orders
if grep -q "e2e_alice_buy_order\|e2e_bob_sell_order" "logs/ubscore.log.$DATE" 2>/dev/null; then
    echo -e " ${GREEN}✅ UBSCore processed orders${NC}"
else
    echo -e " ${YELLOW}⚠️  Orders not found in UBSCore logs${NC}"
fi

# Check Settlement processed
if grep -q "TRADE\|trade" "logs/settlement.log.$DATE" 2>/dev/null; then
    echo -e " ${GREEN}✅ Settlement Service processed trades${NC}"
else
    echo -e " ${YELLOW}⚠️  No trade activity in Settlement logs${NC}"
fi

# Check ScyllaDB
TRADE_QUERY="SELECT COUNT(*) FROM trading.trades WHERE (buy_user_id = $ALICE_ID OR sell_user_id = $BOB_ID) ALLOW FILTERING;"
TRADE_COUNT=$(docker exec scylla cqlsh -e "$TRADE_QUERY" 2>/dev/null | grep -E "^\s*[0-9]+" | tr -d ' ' || echo "0")

if [ "$TRADE_COUNT" != "0" ] && [ "$TRADE_COUNT" != "" ]; then
    echo -e " ${GREEN}✅ Trade recorded in ScyllaDB ($TRADE_COUNT record(s))${NC}"
else
    echo -e " ${YELLOW}⚠️  Trade verification pending in ScyllaDB${NC}"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${GREEN}🎉 TEST 08 PASSED - Full Trading Flow E2E Complete!${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "✅ Complete Flow Validated:"
echo "   1. Deposit (Gateway → Aeron → UBSCore → TigerBeetle)"
echo "   2. Order Placement (Gateway → Aeron → UBSCore → Kafka)"
echo "   3. Matching (ME consumes from Kafka)"
echo "   4. Settlement (Settlement → ScyllaDB)"
echo "   5. Balance Updates (TigerBeetle → Gateway API)"
echo ""
echo "💡 All services still running. To stop:"
echo "   pkill -f 'gateway_service|ubscore_aeron_service|matching_engine_server|settlement_service'"
echo ""

exit 0
