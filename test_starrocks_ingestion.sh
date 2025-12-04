#!/bin/bash

# StarRocks Data Ingestion Verification
# Pushes test trade data to Kafka and verifies StarRocks consumes it

set -e

echo "=========================================="
echo "  StarRocks Data Ingestion Test"
echo "=========================================="

# Step 1: Ensure services are running
echo ""
echo "=== Step 1: Starting required services ==="
docker-compose up -d redpanda starrocks
sleep 5

# Step 2: Wait for StarRocks
echo ""
echo "=== Step 2: Waiting for StarRocks ==="
MAX_RETRIES=30
for i in $(seq 1 $MAX_RETRIES); do
    if docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SELECT 1" > /dev/null 2>&1; then
        echo "✅ StarRocks is ready!"
        break
    fi
    echo "Waiting for StarRocks... ($i/$MAX_RETRIES)"
    sleep 2
done

# Step 3: Initialize schema
echo ""
echo "=== Step 3: Initializing StarRocks schema ==="
./scripts/init_starrocks.sh

# Step 4: Clean data
echo ""
echo "=== Step 4: Cleaning existing data ==="
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "TRUNCATE TABLE settlement.trades;" 2>/dev/null || true

# Step 5: Create Routine Load job
echo ""
echo "=== Step 5: Creating Routine Load job ==="
./scripts/create_starrocks_routine_load.sh

# Step 6: Push test data to Kafka
echo ""
echo "=== Step 6: Pushing test trade data to Kafka ==="

# Create test trade JSON
TEST_TRADE='{
  "output_sequence": 1,
  "trade_id": 1001,
  "match_seq": 1,
  "buy_order_id": 2001,
  "sell_order_id": 2002,
  "buyer_user_id": 1001,
  "seller_user_id": 1002,
  "price": 50000,
  "quantity": 100000000,
  "base_asset": 1,
  "quote_asset": 2,
  "buyer_refund": 0,
  "seller_refund": 0
}'

# Push to Kafka using rpk
echo "$TEST_TRADE" | docker exec -i redpanda rpk topic produce trades

echo "✅ Test trade pushed to Kafka"

# Step 7: Wait for StarRocks to consume
echo ""
echo "=== Step 7: Waiting for StarRocks to consume data ==="
echo "Waiting 10 seconds..."
sleep 10

# Step 8: Check Routine Load status
echo ""
echo "=== Step 8: Checking Routine Load status ==="
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SHOW ROUTINE LOAD FOR settlement.trades_load\G" | grep -E "(State|LoadedRows|ErrorRows|TotalRows)"

# Step 9: Query data from StarRocks
echo ""
echo "=== Step 9: Querying StarRocks for ingested data ==="

TRADE_COUNT=$(docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -N -e "SELECT COUNT(*) FROM settlement.trades;")

echo "Trade count in StarRocks: $TRADE_COUNT"

if [ "$TRADE_COUNT" -gt 0 ]; then
    echo "✅ SUCCESS: Data found in StarRocks!"

    echo ""
    echo "Trade details:"
    docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
    SELECT
        trade_id,
        price,
        quantity,
        buyer_user_id,
        seller_user_id
    FROM settlement.trades
    LIMIT 5;
    "

    echo ""
    echo "=========================================="
    echo "  ✅ StarRocks Ingestion VERIFIED!"
    echo "=========================================="
    echo ""
    echo "Summary:"
    echo "  - Test trade pushed to Kafka ✅"
    echo "  - StarRocks Routine Load consumed data ✅"
    echo "  - Data successfully queried from StarRocks ✅"
    exit 0
else
    echo "❌ FAILED: No data found in StarRocks"
    echo ""
    echo "Debugging info:"
    echo "Routine Load job status:"
    docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SHOW ROUTINE LOAD FOR settlement.trades_load\G"
    exit 1
fi
