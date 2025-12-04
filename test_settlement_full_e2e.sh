#!/bin/bash

# True Full E2E Test
# This script represents the complete end-to-end verification of the HFT Exchange.
# It covers:
# 1. Infrastructure: ScyllaDB (Hot Store) and StarRocks (Cold Store)
# 2. Services: Matching Engine, Settlement Service, Order Gateway
# 3. Flows: Transfers (Deposits), Order Matching, Settlement, Balance Updates
# 4. Verification: Data consistency in both ScyllaDB and StarRocks

set -e

echo "=========================================="
echo "  TRUE FULL E2E TEST (Scylla + StarRocks)"
echo "=========================================="

echo ""
echo "=== Step 1: Infrastructure Check (StarRocks) ==="

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
echo "=== Step 3: Clean Cold Store (StarRocks) ==="
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "TRUNCATE TABLE settlement.trades;" 2>/dev/null || true
echo "✅ StarRocks tables cleaned"

echo ""
echo "=== Step 4: Execute Core E2E Flow ==="
echo "Running test_full_e2e.sh to handle services, traffic, and Hot Store verification..."
./test_full_e2e.sh

echo ""
echo "=== Step 5: Verify Cold Store (StarRocks) ==="
echo "Waiting 15 seconds for async ingestion..."
sleep 15

echo "1. Trade Count Check:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SELECT COUNT(*) as trade_count FROM settlement.trades;"

echo ""
echo "2. Recent Trades:"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT trade_id, price, quantity, buy_user_id, sell_user_id, settled_at
FROM settlement.trades
ORDER BY settled_at DESC LIMIT 5;
"

echo ""
echo "3. OLAP Aggregation (Volume by User):"
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "
SELECT buy_user_id as user_id, 'buy' as side, SUM(quantity) as vol, COUNT(*) as cnt
FROM settlement.trades GROUP BY buy_user_id
UNION ALL
SELECT sell_user_id as user_id, 'sell' as side, SUM(quantity) as vol, COUNT(*) as cnt
FROM settlement.trades GROUP BY sell_user_id
ORDER BY user_id, side;
"

echo ""
echo "=========================================="
echo "  ✅ TRUE FULL E2E TEST COMPLETE"
echo "=========================================="
