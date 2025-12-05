#!/bin/bash

# Complete E2E Test with Transfer In Flow
# This tests the full flow: transfer_in -> deposit -> settlement -> balance updates -> trades

set -e

echo "=========================================="
echo "  Complete E2E Test with Transfer In"
echo "=========================================="

echo ""
echo "=== Step 1: Killing all processes ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
pkill -f order_gate_server || true
pkill -f transfer_server || true
sleep 2

echo ""
echo "=== Step 2: Cleaning all data ==="
rm -rf me_wal_data me_snapshots || true
docker exec scylla cqlsh -e "TRUNCATE settlement.settled_trades;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE settlement.user_balances;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE settlement.ledger_events;" 2>/dev/null || true
rm -f /tmp/*.log
echo "✅ All data cleaned"

echo ""
echo "=== Step 3: Building binaries ==="
cargo build --bin matching_engine_server --bin settlement_service --bin order_gate_server --bin transfer_server 2>&1 | tail -3

echo ""
echo "=== Step 4: Starting services ==="

echo "  4.1 Starting Settlement Service..."
./target/debug/settlement_service > /tmp/settle.log 2>&1 &
SETTLE_PID=$!
sleep 2

echo "  4.2 Starting Matching Engine..."
./target/debug/matching_engine_server > /tmp/me.log 2>&1 &
ME_PID=$!
sleep 3

echo "  4.3 Starting Order Gateway (includes Transfer API)..."
./target/debug/order_gate_server > /tmp/gateway.log 2>&1 &
GATEWAY_PID=$!
sleep 5

echo ""
echo "=== Step 5: Verifying services ==="
if ps -p $SETTLE_PID > /dev/null; then echo "  ✅ Settlement"; else echo "  ❌ Settlement failed"; exit 1; fi
if ps -p $ME_PID > /dev/null; then echo "  ✅ ME"; else echo "  ❌ ME failed"; exit 1; fi
if ps -p $GATEWAY_PID > /dev/null; then echo "  ✅ Gateway"; else echo "  ❌ Gateway failed"; exit 1; fi

echo ""
echo "=== Step 6: Transfer In (Initialize Balances) ==="
# Check if port is open
echo "  Checking if Gateway is listening on 3001..."
lsof -i :3001 || echo "⚠️  Nothing listening on 3001"

# Transfer in for a few test users
for user_id in 1001 1002 1003; do
    echo "  Transferring to user $user_id..."

    # BTC (asset 1)
    curl -v -X POST http://localhost:3001/api/v1/transfer_in \
        -H "Content-Type: application/json" \
        -d "{\"request_id\": \"req_${user_id}_1\", \"user_id\": $user_id, \"asset\": \"BTC\", \"amount\": \"10000.0\"}" 2>&1 | grep -E "(Connected|HTTP)"

    # USDT (asset 2)
    curl -v -X POST http://localhost:3001/api/v1/transfer_in \
        -H "Content-Type: application/json" \
        -d "{\"request_id\": \"req_${user_id}_2\", \"user_id\": $user_id, \"asset\": \"USDT\", \"amount\": \"1000000.0\"}" 2>&1 | grep -E "(Connected|HTTP)"

    # ETH (asset 3)
    curl -v -X POST http://localhost:3001/api/v1/transfer_in \
        -H "Content-Type: application/json" \
        -d "{\"request_id\": \"req_${user_id}_3\", \"user_id\": $user_id, \"asset\": \"ETH\", \"amount\": \"100000.0\"}" 2>&1 | grep -E "(Connected|HTTP)"
done
echo "  ✅ Transfer in completed"

echo ""
echo "=== Step 7: Waiting for settlement (1 seconds) ==="
sleep 1

echo ""
echo "=== Step 8: Verifying initial balances ==="
docker exec scylla cqlsh -e "SELECT user_id, asset_id, available, version FROM settlement.user_balances WHERE user_id IN (1001, 1002, 1003) ALLOW FILTERING;" 2>/dev/null

echo ""
echo "=== Step 8.5: Verify Balance API Response ==="
curl -s "http://localhost:3001/api/user/balance?user_id=1001"
echo ""

echo ""
echo "=== Step 9: Sending test orders (2 seconds) ==="
./target/debug/order_http_client > /tmp/client.log 2>&1 &
CLIENT_PID=$!
sleep 2
kill $CLIENT_PID || true
sleep 2

echo ""
echo "=========================================="
echo "  Test Results"
echo "=========================================="

echo ""
echo "=== Settlement Service Logs (last 30 lines) ==="
tail -30 /tmp/settle.log

echo ""
echo "=== Database Stats ==="
echo "1. Settled Trades:"
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM settlement.settled_trades;" 2>/dev/null

echo ""
echo "2. User Balances (test users):"
docker exec scylla cqlsh -e "SELECT user_id, asset_id, available, frozen, version FROM settlement.user_balances WHERE user_id IN (1001, 1002, 1003) ALLOW FILTERING;" 2>/dev/null

echo ""
echo "3. All User Balances Count:"
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM settlement.user_balances;" 2>/dev/null

echo ""
echo "=== Balance Update Statistics ==="
echo -n "  Deposit balance updates: "
grep -c "Balance updated for deposit" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "  Trade balance updates: "
grep -c "Balances updated for trade" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "  Version conflicts: "
grep -c "Balance update skipped" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "  Balance errors: "
grep -c "Failed to update balances" /tmp/settle.log 2>/dev/null || echo "0"

echo ""
echo "=== Sample Logs ==="
echo "Deposit updates:"
grep "Balance updated for deposit" /tmp/settle.log 2>/dev/null | head -5 || echo "None"
echo ""
echo "Trade updates:"
grep "Balances updated for trade" /tmp/settle.log 2>/dev/null | head -5 || echo "None"

echo ""
echo "=========================================="
echo "  Cleanup"
echo "=========================================="
kill $ME_PID $SETTLE_PID $GATEWAY_PID $TRANSFER_PID 2>/dev/null || true

echo ""
echo "✅ E2E Test Complete!"
echo ""
echo "Expected Results:"
echo "  - 9 deposit balance updates (3 users × 3 assets)"
echo "  - Multiple trade balance updates"
echo "  - User balances should show correct amounts with versions"
echo "  - No balance errors (all updates successful)"
