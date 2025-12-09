#!/bin/bash

# Complete E2E Test with Transfer In Flow
# This tests the full flow: transfer_in -> deposit -> settlement -> balance updates -> trades

set -e

# Determine binary path based on USE_RELEASE
if [ "$USE_RELEASE" = "1" ]; then
    BIN_DIR="./target/release"
    BUILD_FLAG="--release"
else
    BIN_DIR="./target/debug"
    BUILD_FLAG=""
fi

echo "=========================================="
echo "  Complete E2E Test with Transfer In"
echo "=========================================="

echo ""
echo "=== Step 1: Killing all processes ==="
pkill -f matching_engine_server || true
pkill -f settlement_service || true
pkill -f order_gate_server || true
pkill -f ubscore_aeron_service || true
pkill -f transfer_server || true
pkill -f order_http_client || true
sleep 2

echo ""
echo "=== Step 2: Cleaning all data ==="
rm -rf me_wal_data me_snapshots || true
docker exec scylla cqlsh -e "TRUNCATE trading.settled_trades;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE trading.user_balances;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE trading.ledger_events;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE trading.balance_ledger;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE trading.settlement_state;" 2>/dev/null || true
docker exec scylla cqlsh -e "TRUNCATE trading.engine_output_log;" 2>/dev/null || true
rm -f /tmp/*.log
echo "✅ All data cleaned"

echo ""
echo "=== Step 3: Building binaries ==="
cargo build $BUILD_FLAG --bin matching_engine_server --bin settlement_service --bin order_gate_server --bin ubscore_aeron_service 2>&1 | tail -3

echo ""
echo "=== Step 4: Starting services ==="

echo "  4.1 Starting Aeron Media Driver..."
# Check if Aeron is already running
if pgrep -f "aeronmd" > /dev/null; then
    echo "    (Aeron media driver already running)"
else
    # Start Aeron media driver (assuming it's installed)
    aeronmd > /tmp/aeronmd.log 2>&1 &
    AERONMD_PID=$!
    sleep 2
    if ps -p $AERONMD_PID > /dev/null; then
        echo "    ✅ Aeron media driver started"
    else
        echo "    ⚠️  Aeron media driver may not be installed, using system default"
    fi
fi

echo "  4.2 Starting UBSCore Aeron Service..."
rm -rf ~/ubscore_data || true
RUST_LOG=info $BIN_DIR/ubscore_aeron_service > /tmp/ubscore.log 2>&1 &
UBSCORE_PID=$!
sleep 3

echo "  4.3 Starting Settlement Service..."
RUST_LOG=settlement=debug,scylla=warn $BIN_DIR/settlement_service > /tmp/settle.log 2>&1 &
SETTLE_PID=$!
sleep 2

echo "  4.4 Starting Matching Engine..."
$BIN_DIR/matching_engine_server > /tmp/me.log 2>&1 &
ME_PID=$!
sleep 3

echo "  4.5 Starting Order Gateway (includes Transfer API)..."
$BIN_DIR/order_gate_server > /tmp/gateway.log 2>&1 &
GATEWAY_PID=$!
sleep 5

echo ""
echo "=== Step 5: Verifying services ==="
if ps -p $UBSCORE_PID > /dev/null; then echo "  ✅ UBSCore"; else echo "  ❌ UBSCore failed"; cat /tmp/ubscore.log | tail -20; exit 1; fi
if ps -p $SETTLE_PID > /dev/null; then echo "  ✅ Settlement"; else echo "  ❌ Settlement failed"; exit 1; fi
if ps -p $ME_PID > /dev/null; then echo "  ✅ ME"; else echo "  ❌ ME failed"; exit 1; fi
if ps -p $GATEWAY_PID > /dev/null; then echo "  ✅ Gateway"; else echo "  ❌ Gateway failed"; cat /tmp/gateway.log | tail -20; exit 1; fi


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
docker exec scylla cqlsh -e "SELECT user_id, asset_id, avail, version FROM trading.user_balances WHERE user_id IN (1001, 1002, 1003) ALLOW FILTERING;" 2>/dev/null

echo ""
echo "=== Step 8.5: Verify Balance API Response ==="
curl -s "http://localhost:3001/api/user/balance?user_id=1001"
echo ""

echo ""
echo "=== Step 9: Sending test orders (3 seconds) ==="
./target/debug/order_http_client > /tmp/client.log 2>&1 &
CLIENT_PID=$!
sleep 3
kill $CLIENT_PID || true

echo "  Waiting for settlement to catch up (60s)..."
for i in {1..6}; do
    sleep 10
    # Get last progress line
    LAST_PROGRESS=$(grep "\[PROGRESS\]" /tmp/settle.log 2>/dev/null | tail -1)
    PROCESSED=$(echo "$LAST_PROGRESS" | sed 's/.*msgs=\([0-9]*\).*/\1/' 2>/dev/null || echo "0")
    TRADES=$(echo "$LAST_PROGRESS" | sed 's/.*trades=\([0-9]*\).*/\1/' 2>/dev/null || echo "0")
    MSG_OPS=$(echo "$LAST_PROGRESS" | sed 's/.*total: \([0-9]*\)msg.*/\1/' 2>/dev/null || echo "0")
    PUBLISHED=$(grep -c "Published EngineOutput" /tmp/me.log 2>/dev/null || echo "0")
    echo "    Progress: processed=$PROCESSED/$PUBLISHED trades=$TRADES msg_ops=${MSG_OPS}msg/s"
done

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
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.settled_trades;" 2>/dev/null

echo ""
echo "2. User Balances (test users):"
docker exec scylla cqlsh -e "SELECT user_id, asset_id, avail, frozen, version FROM trading.user_balances WHERE user_id IN (1001, 1002, 1003) ALLOW FILTERING;" 2>/dev/null

echo ""
echo "3. All User Balances Count:"
echo "(Note: user_balances table is deprecated, balances are now in balance_ledger)"
docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.balance_ledger;" 2>/dev/null

echo ""
echo "4. Balance Ledger Events (deposits):"
docker exec scylla cqlsh -e "SELECT user_id, asset_id, event_type, delta_avail, avail FROM trading.balance_ledger LIMIT 15;" 2>/dev/null

echo ""
echo "=== Balance Update Statistics ==="
echo -n "  Deposit balance updates: "
grep -c "Balance updated for deposit" /tmp/settle.log 2>/dev/null || echo "0"
echo -n "  Trade balance updates: "
grep -c "Settled trade" /tmp/settle.log 2>/dev/null || echo "0"
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
grep "Settled trade" /tmp/settle.log 2>/dev/null | head -5 || echo "None"

echo ""
echo "=========================================="
echo "  Cleanup"
echo "=========================================="
kill $UBSCORE_PID $ME_PID $SETTLE_PID $GATEWAY_PID 2>/dev/null || true

echo ""
echo "✅ E2E Test Complete!"
echo ""
echo "Expected Results:"
echo "  - 9 deposit balance updates (3 users × 3 assets)"
echo "  - Multiple trade balance updates"
echo "  - User balances should show correct amounts with versions"
echo "  - No balance errors (all updates successful)"
