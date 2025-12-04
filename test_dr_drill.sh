#!/bin/bash
# test_dr_drill.sh

# Cleanup previous runs
echo "Cleaning up..."
rm -f settled_trades.csv failed_trades.json
docker exec scylla cqlsh -e "TRUNCATE settlement.settled_trades;"

# Build binaries
echo "Building binaries..."
cargo build --bin matching_engine_server --bin settlement_service --bin send_orders --bin verify_settlement

# Start Matching Engine
echo "Starting Matching Engine..."
./target/debug/matching_engine_server > me.log 2>&1 &
ME_PID=$!
echo "Started Matching Engine (PID: $ME_PID)"

# Start Settlement Service
echo "Starting Settlement Service..."
./target/debug/settlement_service > settlement.log 2>&1 &
SS_PID=$!
echo "Started Settlement Service (PID: $SS_PID)"

echo "Waiting for services to start..."
sleep 10

# Phase 1: Send 1000 orders
echo "Phase 1: Sending 1000 orders..."
cargo run --bin send_orders -- --count 1000
echo "Waiting for processing..."
sleep 5

# Kill Settlement Service
echo "💥 KILLING SETTLEMENT SERVICE!"
kill -9 $SS_PID
wait $SS_PID 2>/dev/null

# Phase 2: Send 1000 MORE orders (while SS is down)
echo "Phase 2: Sending 1000 orders (while SS is down)..."
cargo run --bin send_orders -- --count 1000
sleep 2

# Restart Settlement Service
echo "♻️ RESTARTING SETTLEMENT SERVICE..."
./target/debug/settlement_service >> settlement.log 2>&1 &
SS_PID_NEW=$!
echo "Restarted Settlement Service (PID: $SS_PID_NEW)"

# Wait for catch-up
echo "Waiting for catch-up (15s)..."
sleep 15

# Verify
echo "Verifying consistency..."
cargo run --bin verify_settlement -- reconcile --file settled_trades.csv

# Cleanup
echo "Stopping services..."
kill $ME_PID $SS_PID_NEW
wait $ME_PID $SS_PID_NEW 2>/dev/null
echo "Done."
