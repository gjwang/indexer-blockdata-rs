#!/bin/bash
set -e

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
KEYSPACE="trading"
GATEWAY_URL="http://127.0.0.1:3001/api/orders"

echo -e "${BLUE}==========================================${NC}"
echo -e "${BLUE}       SCENARIO TEST: FULL TRADE       ${NC}"
echo -e "${BLUE}==========================================${NC}"

# Check dependencies
if ! command -v python3 &> /dev/null; then
    echo -e "${RED}Error: python3 is required${NC}"
    exit 1
fi

# 1. Clean Environment
echo -e "${BLUE}Step 1: Cleaning Environment...${NC}"
pkill -f matching_engine_server || true
pkill -f order_gate_server || true
pkill -f settlement_service || true
pkill -f order_http_client || true

# Init DB
echo "Resetting Database..."
docker exec scylla cqlsh -e "DROP KEYSPACE IF EXISTS $KEYSPACE;" 2>&1 | grep -v "Warnings" || true
docker exec scylla cqlsh -e "CREATE KEYSPACE $KEYSPACE WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};" 2>&1 | grep -v "Warnings" || true
docker exec -i scylla cqlsh -k $KEYSPACE < schema/settlement_unified.cql 2>/dev/null

# Clean Kafka
echo "Resetting Kafka Topics..."
docker exec redpanda rpk topic delete orders trades balance.operations 2>/dev/null || true
docker exec redpanda rpk topic create orders trades balance.operations -p 1 -r 1 2>/dev/null || true
echo "Waiting 5s for Kafka metadata propagation..."
sleep 5

# 2. Start Services
echo -e "${BLUE}Step 2: Starting Services...${NC}"
./target/debug/matching_engine_server > logs/matching_engine.log 2>&1 &
ME_PID=$!
echo "Matching Engine PID: $ME_PID"
sleep 2

./target/debug/order_gate_server > logs/order_gateway.log 2>&1 &
GATE_PID=$!
echo "Order Gateway PID: $GATE_PID"
sleep 2

./target/debug/settlement_service > logs/settlement.log 2>&1 &
SETTLEMENT_PID=$!
echo "Settlement Service PID: $SETTLEMENT_PID"
sleep 2

# 3. Inject Deposits
echo -e "${BLUE}Step 3: Injecting Deposits...${NC}"
TS=$(date +%s)000

# User 1001: Seller. Needs BTC (Asset 1). Amount 1.0 (100,000,000)
# User 1002: Buyer. Needs USDT (Asset 2). Amount 20,000 * 10^8

# JSON for User 1001 (BTC)
DEP1=$(python3 -c "import json; print(json.dumps({'type': 'TransferIn', 'data': {'request_id': 'req-1', 'user_id': 1001, 'asset_id': 1, 'amount': 100000000, 'timestamp': $TS}}))")

# JSON for User 1002 (USDT)
DEP2=$(python3 -c "import json; print(json.dumps({'type': 'TransferIn', 'data': {'request_id': 'req-2', 'user_id': 1002, 'asset_id': 2, 'amount': 2000000000000, 'timestamp': $TS}}))")

echo "Sending Deposit for User 1001 (BTC)..."
echo "$DEP1" | docker exec -i redpanda rpk topic produce balance.operations

echo "Sending Deposit for User 1002 (USDT)..."
echo "$DEP2" | docker exec -i redpanda rpk topic produce balance.operations

echo "Waiting for deposits to process..."
sleep 3

# Verify Balances in Scylla
echo -e "${BLUE}Verifying Initial Balances...${NC}"
docker exec scylla cqlsh -k $KEYSPACE -e "SELECT * FROM user_balances"

# 4. Place Orders via Gateway
echo -e "${BLUE}Step 4: Placing Orders...${NC}"

# User 1001 Sells 0.5 BTC @ 150.0 USDT
# CID generic (Must be 16-32 chars)
CID1="sell-$(date +%s)-ORDER"
ORDER1=$(python3 -c "import json; print(json.dumps({'cid': '$CID1', 'symbol': 'BTC_USDT', 'side': 'Sell', 'order_type': 'Limit', 'price': '150.0', 'quantity': '0.5'}))")

echo "Placing SELL Order (User 1001)..."
# Check Curl output (Verbose)
curl -v -X POST "$GATEWAY_URL?user_id=1001" -H "Content-Type: application/json" -d "$ORDER1"
echo ""

sleep 1

# User 1002 Buys 0.5 BTC @ 150.0 USDT
CID2="buy-$(date +%s)-ORDER"
ORDER2=$(python3 -c "import json; print(json.dumps({'cid': '$CID2', 'symbol': 'BTC_USDT', 'side': 'Buy', 'order_type': 'Limit', 'price': '150.0', 'quantity': '0.5'}))")

echo "Placing BUY Order (User 1002)..."
curl -v -X POST "$GATEWAY_URL?user_id=1002" -H "Content-Type: application/json" -d "$ORDER2"
echo ""

echo "Waiting for Settlement..."
sleep 5

# 5. Verification
echo -e "${BLUE}Step 5: Verifying Final State...${NC}"

echo "--- Active Orders (Should be Empty) ---"
docker exec scylla cqlsh -k $KEYSPACE -e "SELECT * FROM active_orders"

echo "--- Settled Trades (Should have 1 trade) ---"
docker exec scylla cqlsh -k $KEYSPACE -e "SELECT trade_id, buyer_user_id, seller_user_id, price, quantity, settled_at FROM settled_trades"

echo "--- User Balances (Should reflect trade) ---"
docker exec scylla cqlsh -k $KEYSPACE -e "SELECT * FROM user_balances"

echo "--- Order History (Should be Filled) ---"
docker exec scylla cqlsh -k $KEYSPACE -e "SELECT user_id, order_id, side, status, filled_qty FROM order_history"

# Cleanup
echo -e "${BLUE}Stopping Services...${NC}"
kill $ME_PID $GATE_PID $SETTLEMENT_PID
wait $ME_PID $GATE_PID $SETTLEMENT_PID 2>/dev/null || true

echo -e "${GREEN}Test Complete${NC}"
