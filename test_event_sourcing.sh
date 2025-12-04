#!/bin/bash
# test_event_sourcing.sh

# Ensure binaries are built
echo "Building binaries..."
cargo build --bin matching_engine_server --bin settlement_service --bin transfer_server --bin balance_test_client

# Environment variables
export RUST_LOG=info
export APP__ENABLE_LOCAL_WAL=false

# Start Matching Engine
echo "Starting Matching Engine..."
./target/debug/matching_engine_server > me.log 2>&1 &
ME_PID=$!

# Start Settlement Service
echo "Starting Settlement Service..."
./target/debug/settlement_service > settlement.log 2>&1 &
SS_PID=$!

# Start Transfer Server
echo "Starting Transfer Server..."
./target/debug/transfer_server > transfer.log 2>&1 &
TS_PID=$!

echo "Waiting for services to start..."
sleep 15

# Run Balance Test Client
echo "Running Balance Test Client..."
./target/debug/balance_test_client > client.log 2>&1

echo "Waiting for processing..."
sleep 5

# Query ScyllaDB for Ledger Events
echo "=== Ledger Events in ScyllaDB ==="
docker exec scylla cqlsh -e "SELECT user_id, sequence_id, event_type, amount, currency, created_at FROM settlement.ledger_events;"

# Cleanup
echo "Stopping services..."
kill $ME_PID $SS_PID $TS_PID
wait $ME_PID $SS_PID $TS_PID 2>/dev/null

echo "Done."
