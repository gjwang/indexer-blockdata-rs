#!/bin/bash
set -e

echo "=== Building Binaries ==="
cargo build --bin matching_engine_server
cargo build --bin order_http_client
cargo build --bin order_gate_server
cargo build --bin trade_history_consumer

echo "=== Cleanup ==="
pkill -f matching_engine_server || true
pkill -f order_gate_server || true
rm -rf me_wal_1 me_wal_2 me_snap_1 me_snap_2
rm -f node1.log node2.log trades1.txt trades2.txt

echo "=== Configuring Kafka (1 Partition for Total Ordering) ==="
docker exec redpanda rpk topic delete orders || true
docker exec redpanda rpk topic create orders -p 1
sleep 2

echo "=== Starting Order Gateway ==="
./target/debug/order_gate_server > gateway.log 2>&1 &
PID_GW=$!
sleep 5

echo "=== Starting Consumer 1 (Listening to trades_1) ==="
APP__KAFKA__TOPICS__TRADES=trades_1 ./target/debug/trade_history_consumer > consumer1.log 2>&1 &
PID_C1=$!

echo "=== Starting Consumer 2 (Listening to trades_2) ==="
APP__KAFKA__TOPICS__TRADES=trades_2 ./target/debug/trade_history_consumer > consumer2.log 2>&1 &
PID_C2=$!

echo "=== Starting Node 1 (Leader) -> trades_1 ==="
GID1="me_node_$(date +%s)_1"
APP__ENABLE_LOCAL_WAL=false APP__KAFKA__TOPICS__TRADES=trades_1 APP__KAFKA__GROUP_ID=$GID1 APP_WAL_DIR=me_wal_1 APP_SNAP_DIR=me_snap_1 ./target/debug/matching_engine_server > node1.log 2>&1 &
PID1=$!
echo "Node 1 PID: $PID1 (Group: $GID1)"

echo "=== Starting Node 2 (Follower) -> trades_2 ==="
GID2="me_node_$(date +%s)_2"
APP__ENABLE_LOCAL_WAL=false APP__KAFKA__TOPICS__TRADES=trades_2 APP__KAFKA__GROUP_ID=$GID2 APP_WAL_DIR=me_wal_2 APP_SNAP_DIR=me_snap_2 ./target/debug/matching_engine_server > node2.log 2>&1 &
PID2=$!
echo "Node 2 PID: $PID2 (Group: $GID2)"

echo "Waiting for nodes to start..."
sleep 5

DURATION=${1:-10}
echo "=== Sending Orders for $DURATION seconds ==="
# Run client for DURATION seconds
perl -e "alarm $DURATION; exec @ARGV" "./target/debug/order_http_client" > client.log 2>&1 || true

echo "Waiting for processing (60s)..."
sleep 60

echo "=== Verifying Determinism ==="
# Extract Trade lines from CONSUMER logs
grep -a "Trade #" consumer1.log | awk '{$1=""; print $0}' | sort > trades1.txt
grep -a "Trade #" consumer2.log | awk '{$1=""; print $0}' | sort > trades2.txt

COUNT1=$(wc -l < trades1.txt)
COUNT2=$(wc -l < trades2.txt)

echo "Node 1 Trades: $COUNT1"
echo "Node 2 Trades: $COUNT2"

if [ "$COUNT1" -eq 0 ]; then
    echo "ERROR: No trades generated!"
    cat node1.log | tail -20
    kill $PID1 $PID2 $PID_C1 $PID_C2
    exit 1
fi

# Allow diff to fail (exit code 1 means difference)
DIFF=$(diff trades1.txt trades2.txt || true)
if [ -z "$DIFF" ]; then
    echo "✅ SUCCESS: Both nodes produced IDENTICAL trades!"
    echo "Sample Output:"
    head -5 trades1.txt
else
    echo "❌ FAILURE: Outputs differ!"
    echo "First 20 lines of diff:"
    echo "$DIFF" | head -20
    echo "=== Node 1 Log Tail ==="
    tail -20 node1.log
    echo "=== Node 2 Log Tail ==="
    tail -20 node2.log
fi

echo "=== Cleanup ==="
kill $PID1 $PID2 $PID_C1 $PID_C2 $PID_GW
