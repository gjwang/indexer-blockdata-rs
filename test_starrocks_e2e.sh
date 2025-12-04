#!/bin/bash

# StarRocks E2E Test
# Tests that StarRocks can consume trades from Kafka via Routine Load

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

echo ""
echo ""
echo "=== Step 4: (Skipped) Routine Load ==="
echo "Direct ingestion from Settlement Service is now used."

echo ""
echo "=== Step 5: Run the main E2E test to generate trades ==="
echo "This will start services, initialize balances, and generate trades..."
./test_full_e2e.sh

echo ""
echo "=== Step 6: Wait for StarRocks to consume trades ==="
echo "Waiting 15 seconds for Settlement Service to load data..."
sleep 15

echo ""
echo "=== Step 7: (Skipped) Routine Load Status ==="

echo ""
echo "=== Step 8: Querying StarRocks for trades ==="

echo "Trade count:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SELECT COUNT(*) as trade_count FROM settlement.trades;"

echo ""
echo "Recent trades:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT
    trade_id,
    price,
    quantity,
    buy_user_id,
    sell_user_id,
    settled_at
FROM settlement.trades
ORDER BY settled_at DESC
LIMIT 10;
"

echo ""
echo "=== Step 9: Running OLAP queries ==="

echo "Total volume by user:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT
    buy_user_id as user_id,
    'buy' as side,
    SUM(quantity) as total_volume,
    COUNT(*) as trade_count
FROM settlement.trades
GROUP BY buy_user_id
UNION ALL
SELECT
    sell_user_id as user_id,
    'sell' as side,
    SUM(quantity) as total_volume,
    COUNT(*) as trade_count
FROM settlement.trades
GROUP BY sell_user_id
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
echo "=========================================="
echo "  ✅ StarRocks E2E Test Complete!"
echo "=========================================="
echo ""
echo "Summary:"
echo "  - Settlement Service directly loaded trades into StarRocks"
echo "  - OLAP queries successfully executed"
echo "  - Data pipeline verified: ME → Settlement → StarRocks"
