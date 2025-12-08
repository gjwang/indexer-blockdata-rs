#!/bin/bash
#
# UBSCore Latency Test Script
# Tests Gateway → UBSCore latency (validate + WAL)
#

set -e

GATEWAY_PORT=3001
LOG_FILE="/tmp/gateway_test_$$.log"
NUM_ORDERS=${1:-100}

echo "=========================================="
echo "  UBSCore Latency Test"
echo "=========================================="
echo ""

# Step 1: Clean up any existing gateway and old data
echo "[1/4] Cleaning up..."
killall order_gate_server 2>/dev/null || true
sleep 1

# Delete old log files
rm -f /tmp/gateway_test_*.log /tmp/latencies_*.txt
# Reset WAL for clean test
rm -f ~/gateway_data/gateway_ubs.wal
echo "      Cleaned processes, logs, and WAL"
sleep 1

# Step 2: Start gateway with logging
echo "[2/4] Starting gateway..."
RUST_LOG=info ./target/release/order_gate_server > "$LOG_FILE" 2>&1 &
GATEWAY_PID=$!
echo "      PID: $GATEWAY_PID, Log: $LOG_FILE"

# Wait for startup
sleep 2
if ! kill -0 $GATEWAY_PID 2>/dev/null; then
    echo "ERROR: Gateway failed to start"
    cat "$LOG_FILE"
    exit 1
fi
echo "      Gateway running"

# Step 3: Send orders
echo "[3/4] Sending $NUM_ORDERS orders..."
for i in $(seq 1 $NUM_ORDERS); do
    curl -s -X POST "http://localhost:$GATEWAY_PORT/api/orders?user_id=1001" \
        -H "Content-Type: application/json" \
        -d '{"symbol":"BTC_USDT","side":"Buy","price":"50000.00","quantity":"0.01","order_type":"Limit"}' > /dev/null
    if [ $((i % 20)) -eq 0 ]; then
        echo -ne "\r      Progress: $i/$NUM_ORDERS"
    fi
done
echo -e "\r      Progress: $NUM_ORDERS/$NUM_ORDERS - Done!"

# Step 4: Analyze latency
echo "[4/4] Analyzing UBSCore latency..."
sleep 1

# Kill gateway gracefully
kill $GATEWAY_PID 2>/dev/null || true
sleep 1

# Extract latencies
grep "validate+wal" "$LOG_FILE" | sed 's/.*validate+wal=\([0-9]*\).*/\1/' | sort -n > /tmp/latencies_$$.txt
COUNT=$(wc -l < /tmp/latencies_$$.txt | tr -d ' ')

if [ "$COUNT" -eq 0 ]; then
    echo "ERROR: No latency data found"
    exit 1
fi

echo ""
echo "=========================================="
echo "  Results (UBSCore: validate + WAL)"
echo "=========================================="
echo ""

# Calculate stats
MIN=$(head -1 /tmp/latencies_$$.txt)
MAX=$(tail -1 /tmp/latencies_$$.txt)
AVG=$(awk '{sum+=$1} END {print int(sum/NR)}' /tmp/latencies_$$.txt)
P50=$(sed -n "$((COUNT * 50 / 100))p" /tmp/latencies_$$.txt)
P95=$(sed -n "$((COUNT * 95 / 100))p" /tmp/latencies_$$.txt)
P99=$(sed -n "$((COUNT * 99 / 100))p" /tmp/latencies_$$.txt)

echo "Samples:  $COUNT"
echo ""
echo "Avg:      ${AVG}µs ($(echo "scale=2; $AVG/1000" | bc)ms)"
echo "Min:      ${MIN}µs ($(echo "scale=2; $MIN/1000" | bc)ms)"
echo "Max:      ${MAX}µs ($(echo "scale=2; $MAX/1000" | bc)ms)"
echo ""
echo "P50:      ${P50}µs ($(echo "scale=2; $P50/1000" | bc)ms)"
echo "P95:      ${P95}µs ($(echo "scale=2; $P95/1000" | bc)ms)"
echo "P99:      ${P99}µs ($(echo "scale=2; $P99/1000" | bc)ms)"
echo ""
echo "=========================================="

# Cleanup
rm -f /tmp/latencies_$$.txt
