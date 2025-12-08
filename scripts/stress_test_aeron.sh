#!/bin/bash
#
# Aeron Stress Test
# Measures latency for Gateway -> Aeron -> UBSCore -> Aeron -> Gateway round-trip
#

set -e

# Configuration
BASE_URL="http://localhost:3001"
NUM_ORDERS=${1:-100}      # Default 100 orders
CONCURRENCY=${2:-1}       # Default 1 concurrent

echo "============================================"
echo "    Aeron IPC Stress Test"
echo "============================================"
echo "Orders: $NUM_ORDERS"
echo "Concurrency: $CONCURRENCY"
echo ""

# Check if services are running
if ! curl -s "$BASE_URL" >/dev/null 2>&1; then
    echo "❌ Gateway not running on $BASE_URL"
    echo "   Start with: ./target/release/order_gate_server"
    exit 1
fi

echo "✅ Gateway running"
echo ""

# Generate unique CIDs and send orders
echo "Starting stress test..."
echo ""

LATENCIES=()
START_TIME=$(python3 -c "import time; print(int(time.time() * 1000))")

for i in $(seq 1 $NUM_ORDERS); do
    CID=$(printf "stress_test_%012d" $i)
    USER_ID=$((1001 + (i % 10)))
    SIDE=$( [ $((i % 2)) -eq 0 ] && echo "Buy" || echo "Sell" )
    PRICE=$((49000 + (RANDOM % 2000)))
    QTY="0.$(printf '%03d' $((100 + RANDOM % 400)))"

    # Time the request
    REQ_START=$(python3 -c "import time; print(time.time() * 1000)")

    RESPONSE=$(curl -s -w "\n%{time_total}" \
        "${BASE_URL}/api/orders?user_id=${USER_ID}" \
        -X POST \
        -H "Content-Type: application/json" \
        -d "{\"symbol\":\"BTC_USDT\",\"side\":\"${SIDE}\",\"order_type\":\"Limit\",\"price\":\"${PRICE}\",\"quantity\":\"${QTY}\",\"cid\":\"${CID}\"}")

    # Extract response body and time
    BODY=$(echo "$RESPONSE" | head -1)
    TIME_TOTAL=$(echo "$RESPONSE" | tail -1)
    TIME_MS=$(python3 -c "print(int(${TIME_TOTAL} * 1000))")

    # Check for success
    if echo "$BODY" | grep -q '"status":0'; then
        STATUS="✓"
    else
        STATUS="✗"
        echo "  Failed: $BODY"
    fi

    LATENCIES+=($TIME_MS)

    # Progress every 10 orders
    if [ $((i % 10)) -eq 0 ] || [ $i -eq $NUM_ORDERS ]; then
        echo "  [$i/$NUM_ORDERS] Last: ${TIME_MS}ms $STATUS"
    fi
done

END_TIME=$(python3 -c "import time; print(int(time.time() * 1000))")
TOTAL_TIME=$((END_TIME - START_TIME))

echo ""
echo "============================================"
echo "    Results"
echo "============================================"

# Calculate statistics
# Convert bash array to comma-separated for Python
LATENCIES_CSV=$(IFS=,; echo "${LATENCIES[*]}")

python3 << EOF
import sys

latencies = [${LATENCIES_CSV}]
latencies.sort()

n = len(latencies)
total_ms = $TOTAL_TIME
qps = n / (total_ms / 1000)

print(f"Orders processed: {n}")
print(f"Total time:       {total_ms}ms")
print(f"Throughput:       {qps:.1f} orders/sec")
print()
print("Latency (round-trip HTTP -> Aeron -> UBSCore -> HTTP):")
print(f"  Min:     {latencies[0]}ms")
print(f"  Max:     {latencies[-1]}ms")
print(f"  Avg:     {sum(latencies) / n:.1f}ms")
print(f"  P50:     {latencies[n // 2]}ms")
print(f"  P90:     {latencies[int(n * 0.9)]}ms")
print(f"  P99:     {latencies[int(n * 0.99)]}ms" if n >= 100 else "")
EOF

echo ""
echo "Done!"
