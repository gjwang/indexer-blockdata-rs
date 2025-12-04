#!/bin/bash

# StarRocks E2E Test
# Tests the full flow: trades -> Kafka -> StarRocks Routine Load -> OLAP queries

set -e

echo "=========================================="
echo "  StarRocks E2E Test"
echo "=========================================="

echo ""
echo "=== Step 1: Check Prerequisites ==="

# Check if StarRocks is running
if ! docker ps | grep -q starrocks; then
    echo "❌ StarRocks container is not running"
    echo "Starting StarRocks..."
    docker-compose up -d starrocks
    echo "Waiting for StarRocks to be ready..."
    sleep 30
fi

# Wait for StarRocks to be ready
MAX_RETRIES=30
for i in $(seq 1 $MAX_RETRIES); do
    if docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SELECT 1" > /dev/null 2>&1; then
        echo "✅ StarRocks is ready!"
        break
    fi
    echo "Waiting for StarRocks... ($i/$MAX_RETRIES)"
    sleep 2
done

echo ""
echo "=== Step 2: Initialize StarRocks Schema ==="
./scripts/init_starrocks.sh

echo ""
echo "=== Step 3: Clean existing data ==="
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "TRUNCATE TABLE settlement.trades;" 2>/dev/null || true
echo "✅ StarRocks tables cleaned"

# Clean Kafka topic
docker exec redpanda rpk topic delete trades 2>/dev/null || true
docker exec redpanda rpk topic create trades --partitions 3 --replicas 1 2>/dev/null || true
echo "✅ Kafka topic recreated"

echo ""
echo "=== Step 4: Create StarRocks Routine Load Job ==="
./scripts/create_starrocks_routine_load.sh

echo ""
echo "=== Step 5: Killing existing processes ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
sleep 2

echo ""
echo "=== Step 6: Cleaning matching engine data ==="
rm -rf me_wal_data me_snapshots || true
docker exec -i scylla cqlsh -e "TRUNCATE settlement.settled_trades;" 2>/dev/null || true
docker exec -i scylla cqlsh -e "TRUNCATE settlement.user_balances;" 2>/dev/null || true
echo "✅ Matching engine data cleaned"

echo ""
echo "=== Step 7: Building binaries ===="
cargo build --bin matching_engine_server --bin settlement_service 2>&1 | tail -3

echo ""
echo "=== Step 8: Starting services ===="

echo "  8.1 Starting Settlement Service..."
./target/debug/settlement_service > /tmp/settle_sr.log 2>&1 &
SETTLE_PID=$!
sleep 2

echo "  8.2 Starting Matching Engine..."
./target/debug/matching_engine_server > /tmp/me_sr.log 2>&1 &
ME_PID=$!
sleep 3

echo "✅ Services started (Settlement PID: $SETTLE_PID, ME PID: $ME_PID)"

echo ""
echo "=== Step 9: Initializing user balances ===="

# Initialize balances via settlement service
curl -s -X POST http://localhost:8083/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 1001,
    "asset_id": 1,
    "amount": 1000000000000,
    "tx_id": "init_btc_1001"
  }' > /dev/null

curl -s -X POST http://localhost:8083/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 1001,
    "asset_id": 2,
    "amount": 100000000000000,
    "tx_id": "init_usdt_1001"
  }' > /dev/null

curl -s -X POST http://localhost:8083/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 1002,
    "asset_id": 1,
    "amount": 1000000000000,
    "tx_id": "init_btc_1002"
  }' > /dev/null

curl -s -X POST http://localhost:8083/deposit \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 1002,
    "asset_id": 2,
    "amount": 100000000000000,
    "tx_id": "init_usdt_1002"
  }' > /dev/null

sleep 2
echo "✅ User balances initialized"

echo ""
echo "=== Step 10: Sending test orders ===="

# Send buy order from user 1001
ORDER_1=$(curl -s -X POST http://localhost:8082/order \
  -H "Content-Type: application/json" \
  -d '{
    "symbol": "BTC_USDT",
    "side": "buy",
    "order_type": "limit",
    "price": "50000.00",
    "quantity": "0.5",
    "user_id": 1001
  }')

# Send sell order from user 1002
ORDER_2=$(curl -s -X POST http://localhost:8082/order \
  -H "Content-Type: application/json" \
  -d '{
    "symbol": "BTC_USDT",
    "side": "sell",
    "order_type": "limit",
    "price": "50000.00",
    "quantity": "0.5",
    "user_id": 1002
  }')

echo "✅ Orders sent"
echo "  Order 1: $ORDER_1"
echo "  Order 2: $ORDER_2"

echo ""
echo "=== Step 11: Waiting for trades to be processed ===="
sleep 10

echo ""
echo "=== Step 12: Checking StarRocks Routine Load Status ===="
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SHOW ROUTINE LOAD\G" | grep -E "(State|CurrentTaskNum|ErrorRows|TotalRows)"

echo ""
echo "=== Step 13: Querying StarRocks for trades ===="

echo "Trade count:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SELECT COUNT(*) as trade_count FROM settlement.trades;"

echo ""
echo "Recent trades:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT
    trade_id,
    price,
    quantity,
    buyer_user_id,
    seller_user_id,
    settled_at
FROM settlement.trades
ORDER BY settled_at DESC
LIMIT 10;
"

echo ""
echo "=== Step 14: Running OLAP queries ===="

echo "Total volume by user:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT
    buyer_user_id as user_id,
    'buy' as side,
    SUM(quantity) as total_volume,
    COUNT(*) as trade_count
FROM settlement.trades
GROUP BY buyer_user_id
UNION ALL
SELECT
    seller_user_id as user_id,
    'sell' as side,
    SUM(quantity) as total_volume,
    COUNT(*) as trade_count
FROM settlement.trades
GROUP BY seller_user_id
ORDER BY user_id, side;
"

echo ""
echo "Price statistics:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT
    MIN(price) as min_price,
    MAX(price) as max_price,
    AVG(price) as avg_price,
    SUM(quantity) as total_volume
FROM settlement.trades;
"

echo ""
echo "==========================================
"
echo "  Cleanup"
echo "=========================================="

kill $ME_PID $SETTLE_PID 2>/dev/null || true
sleep 1

echo ""
echo "✅ StarRocks E2E Test Complete!"
echo ""
echo "Expected Results:"
echo "  - Routine Load job should be in RUNNING state"
echo "  - Trades should appear in settlement.trades table"
echo "  - OLAP queries should return aggregated statistics"
echo "  - No error rows in Routine Load"
