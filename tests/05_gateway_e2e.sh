#!/bin/bash
set -e

# Cleanup on exit
cleanup() {
    echo "🧹 Cleanup..."
    # Kill check_service pids if any (not strictly needed if main exits)
    # But mainly kill the background cargo run / binaries
    pkill -P $$ || true
    # Also explicit kill by name to be sure
    pkill -f "order_gate_server|ubscore_aeron_service|settlement_service" || true
}
trap cleanup EXIT

# E2E Test: HTTP Client -> Gateway -> (Kafka) -> UBSCore
# Verifies Deposits (Transfer In) flow which uses Kafka topic 'balance_ops'.

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'


# 0. Build Binaries (Fail Fast)
echo "� Building binaries..."
cargo build --bin ubscore_aeron_service --features aeron
cargo build --bin order_gate_server --features aeron
cargo build --bin settlement_service

# 1. Clean & Setup Environment
echo "🧹 Cleaning environment..."
pkill -f "order_gate_server|ubscore_aeron_service|settlement_service" || true
rm -rf ~/ubscore_data
docker rm -f tigerbeetle || true
docker-compose down -v
rm -rf data/tigerbeetle/*
mkdir -p data/tigerbeetle
rm -f *.log
rm -rf logs/*
mkdir -p logs

echo "🐳 Starting Infrastructure..."
docker-compose up -d --remove-orphans

echo -n "⏳ Waiting for ScyllaDB (9042)..."
timeout=60
count=0
while [ $count -lt $timeout ]; do
    if nc -z localhost 9042; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    sleep 1
    let count=count+1
    echo -n "."
done
echo "🟢 Scylla TCP Open. Waiting 30s for CQL init..."
sleep 30 # Extra buffer for Scylla initialization

echo "📜 Applying Scylla Schema..."
docker cp schema/settlement_unified.cql scylla:/tmp/schema.cql
docker exec scylla cqlsh -f /tmp/schema.cql
sleep 3 # Wait for schema propagation

# Setup TigerBeetle
echo "🐯 Setting up TigerBeetle..."
docker run --privileged --rm -v $(pwd)/data/tigerbeetle:/data ghcr.io/tigerbeetle/tigerbeetle:latest format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle > /dev/null
docker run --privileged -d --name tigerbeetle -p 3000:3000 -v $(pwd)/data/tigerbeetle:/data ghcr.io/tigerbeetle/tigerbeetle:latest start --addresses=0.0.0.0:3000 /data/0_0.tigerbeetle > /dev/null

# 2. Start UBSCore Service (Aeron Mode)
echo "▶️  Starting UBSCore Service (Aeron)..."
# Start with SEED_TEST_ACCOUNTS=0 to avoid noise
SEED_TEST_ACCOUNTS=0 ./target/debug/ubscore_aeron_service --features aeron > logs/ubscore_std.log 2>&1 &
UBS_PID=$!

# Determine correct log file
DATE=$(date +%Y-%m-%d)
LOG_FILE="logs/ubscore.log.$DATE"

echo -n "⏳ Waiting for UBSCore (checking $LOG_FILE)..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if grep -q "UBSCore Service ready" "$LOG_FILE"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    # Check if process died
    if ! kill -0 $UBS_PID 2>/dev/null; then
         echo -e " ${RED}UBSCore died!${NC}"
         cat logs/ubscore_std.log
         exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# 3. Start Gateway Service
echo "▶️  Starting Gateway Service..."
# Gateway connects to UBSCore via Aeron (UDP)
./target/debug/order_gate_server --features aeron > logs/gateway_std.log 2>&1 &
GW_PID=$!
GW_LOG="logs/gateway.log.$DATE"

echo -n "⏳ Waiting for Gateway..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if grep -q "Gateway starting" "$GW_LOG"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    if ! kill -0 $GW_PID 2>/dev/null; then
         echo -e " ${RED}Gateway died!${NC}"
         cat logs/gateway_std.log
         exit 1
    fi
    if ! kill -0 $UBS_PID 2>/dev/null; then
         echo "UBSCore died!"
         exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# 4. Start Settlement Service
echo "▶️  Starting Settlement Service..."
./target/debug/settlement_service > logs/settlement_std.log 2>&1 &
SETTLE_PID=$!
SETTLE_LOG="logs/settlement.log.$DATE"

echo -n "⏳ Waiting for Settlement (checking $SETTLE_LOG)..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if grep -q "Settlement Service Ready" "$SETTLE_LOG"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    if ! kill -0 $SETTLE_PID 2>/dev/null; then
         echo "Settlement died!"
         tail -n 10 "$SETTLE_LOG"
         exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

# 5. Run Test: Deposit (Transfer In)
echo "🧪 Testing Deposit (Transfer In)..."

# User 5001, Asset 2 (USDT), Amount 1000
# Request ID: req_5001_1
REQ='{"request_id": "req_5001_1", "user_id": 5001, "asset": "USDT", "amount": "1000.0"}'

echo "👉 Sending: $REQ"
curl -s -X POST http://localhost:3001/api/v1/user/transfer_in \
    -H "Content-Type: application/json" \
    -d "$REQ" | jq .

# Verify Log in UBSCore
echo "👀 Verifying UBSCore processed the deposit..."
sleep 2

if grep -q "DEPOSIT_EXIT" "$LOG_FILE"; then
     echo -e " ${GREEN}✅ UBSCore Successfully Processed Deposit${NC}"
else
     echo -e " ${RED}❌ UBSCore Missing Deposit Log${NC}"
     echo "Tail of $LOG_FILE:"
     tail -n 20 "$LOG_FILE"
     kill $UBS_PID $GW_PID $SETTLE_PID
     exit 1
fi

# 6. Verify Balance via Gateway
echo "👀 Verifying Balance via Gateway..."
# Wait for Settlement -> Scylla
echo "Waiting 5s for Settlement..."
sleep 5

BALANCE_RES=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=5001")
echo "Result: $BALANCE_RES"

# Check if balance reflects 1000
if echo "$BALANCE_RES" | grep -q "1000"; then
    echo -e " ${GREEN}✅ Balance Verified via Gateway${NC}"
else
     echo -e " ${RED}❌ Balance mismatch${NC}"
     # DB might be async (Materialized View), wait a bit?
     echo "Wait 5s..."
     sleep 5
     BALANCE_RES=$(curl -s "http://localhost:3001/api/v1/user/balance?user_id=5001")
     echo "Result: $BALANCE_RES"

     if echo "$BALANCE_RES" | grep -q "1000"; then
        echo -e " ${GREEN}✅ Balance Verified via Gateway${NC}"
     else
        echo -e " ${RED}❌ FATAL: Balance mismatch${NC}"
        kill $UBS_PID $GW_PID $SETTLE_PID
        exit 1
     fi
fi

echo -e "\n${GREEN}🎉 E2E Test Passed${NC}"
kill $UBS_PID $GW_PID $SETTLE_PID
exit 0
