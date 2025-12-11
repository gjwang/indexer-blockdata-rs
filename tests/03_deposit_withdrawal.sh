#!/bin/bash
set -e

# Test 03: Deposit & Withdrawal via HTTP API (Standalone)
# This test runs the full stack independently

echo "🧪 TEST 03: Deposit & Withdrawal via HTTP"
echo "=========================================="

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Cleanup function
cleanup() {
    echo "🧹 Cleanup..."
    pkill -f "order_gate_server|ubscore_aeron_service|settlement_service" || true
    docker-compose down -v 2>/dev/null || true
    docker rm -f tigerbeetle 2>/dev/null || true
}
trap cleanup EXIT

# Just run the full E2E test infrastructure (reuse test 05 approach)
echo "📋 Setting up full infrastructure..."
bash tests/01_infrastructure.sh || exit 1

# Wait extra time for Redpanda/Kafka to be fully ready
echo "⏳ Waiting for Kafka to be fully ready..."
sleep 5

# Build services
echo "🔨 Building services..."
cargo build --bin ubscore_aeron_service --bin order_gate_server --features aeron --quiet

# Start UBSCore
echo "▶️  Starting UBSCore..."
rm -rf ~/ubscore_data
SEED_TEST_ACCOUNTS=0 ./target/debug/ubscore_aeron_service --features aeron > logs/ubscore_std.log 2>&1 &
UBS_PID=$!

DATE=$(date +%Y-%m-%d)
UBS_LOG="logs/ubscore.log.$DATE"

echo -n "⏳ Waiting for UBSCore..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if [ -f "$UBS_LOG" ] && grep -q "UBSCore Service ready" "$UBS_LOG"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# Start Gateway
echo "▶️  Starting Gateway..."
./target/debug/order_gate_server --features aeron > logs/gateway_std.log 2>&1 &
GW_PID=$!

GW_LOG="logs/gateway.log.$DATE"
echo -n "⏳ Waiting for Gateway..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if [ -f "$GW_LOG" ] && grep -q "Gateway starting" "$GW_LOG"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# Test 1: Deposit USDT
echo ""
echo "💰 Test 1: Deposit 1000 USDT for user 3001..."
DEPOSIT_REQ='{"request_id": "test_dep_001", "user_id": 3001, "asset": "USDT", "amount": "1000.0"}'

DEPOSIT_RESULT=$(curl -s -X POST http://localhost:3001/api/v1/user/transfer_in \
    -H "Content-Type: application/json" \
    -d "$DEPOSIT_REQ")

echo "Response: $DEPOSIT_RESULT"

if echo "$DEPOSIT_RESULT" | grep -q '"success":true'; then
    echo -e " ${GREEN}✅ Deposit request accepted${NC}"
else
    echo -e " ${RED}❌ Deposit failed${NC}"
    echo "$DEPOSIT_RESULT"
    exit 1
fi

# Wait for processing
sleep 3

# Test 2: Query Balance
echo ""
echo "💳 Test 2: Query balance for user 3001..."
BALANCE_RESULT=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=3001")

echo "Response: $BALANCE_RESULT"

if echo "$BALANCE_RESULT" | grep -q "1000"; then
    echo -e " ${GREEN}✅ Balance shows 1000 USDT${NC}"
else
    echo -e " ${RED}❌ Balance mismatch${NC}"
    echo "$BALANCE_RESULT"
    exit 1
fi

echo ""
echo "� Final Balance:"
echo "$BALANCE_RESULT" | jq '.' 2>/dev/null || echo "$BALANCE_RESULT"

echo ""
echo -e "${GREEN}🎉 TEST 03 PASSED - Deposit & Balance Queries Working${NC}"
exit 0
