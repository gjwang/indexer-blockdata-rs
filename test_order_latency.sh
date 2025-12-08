#!/bin/bash
#
# Order Latency Test Script
# Tests Gateway → UBSCore → Kafka order flow latency
#

set -e

# Configuration
GATEWAY_URL="${GATEWAY_URL:-http://localhost:3001}"
USER_ID="${USER_ID:-1001}"
NUM_ORDERS="${1:-100}"  # Default 100 orders, or pass as argument

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo "=========================================="
echo "  Order Latency Test"
echo "=========================================="
echo ""
echo "Gateway:    $GATEWAY_URL"
echo "User ID:    $USER_ID"
echo "Orders:     $NUM_ORDERS"
echo ""

# Check if gateway is running
if ! curl -s "$GATEWAY_URL/api/user/balance?user_id=1001" > /dev/null 2>&1; then
    echo -e "${YELLOW}⚠ Gateway not responding. Start with:${NC}"
    echo "  ./target/release/order_gate_server"
    exit 1
fi
echo -e "${GREEN}✓${NC} Gateway is running"
echo ""

# Warmup (first request is always slow)
echo "Warming up..."
curl -s -X POST "$GATEWAY_URL/api/orders?user_id=$USER_ID" \
    -H "Content-Type: application/json" \
    -d '{"symbol":"BTC_USDT","side":"Buy","price":"50000.00","quantity":"0.01","order_type":"Limit"}' > /dev/null
sleep 0.5

echo ""
echo "Running latency test ($NUM_ORDERS orders)..."
echo ""

# Run test
SUM=0
MIN=999999999
MAX=0
LATENCIES=""

for i in $(seq 1 $NUM_ORDERS); do
    START=$(python3 -c "import time; print(int(time.time()*1000000))")

    RESPONSE=$(curl -s -X POST "$GATEWAY_URL/api/orders?user_id=$USER_ID" \
        -H "Content-Type: application/json" \
        -d '{"symbol":"BTC_USDT","side":"Buy","price":"50000.00","quantity":"0.01","order_type":"Limit"}')

    END=$(python3 -c "import time; print(int(time.time()*1000000))")
    LATENCY=$((END - START))

    SUM=$((SUM + LATENCY))
    LATENCIES="$LATENCIES $LATENCY"

    if [ $LATENCY -lt $MIN ]; then MIN=$LATENCY; fi
    if [ $LATENCY -gt $MAX ]; then MAX=$LATENCY; fi

    # Progress indicator
    if [ $((i % 10)) -eq 0 ]; then
        echo -ne "\r  Progress: $i/$NUM_ORDERS"
    fi
done

echo -e "\r  Progress: $NUM_ORDERS/$NUM_ORDERS - Done!"
echo ""

# Calculate stats
AVG=$((SUM / NUM_ORDERS))
AVG_MS=$(echo "scale=2; $AVG/1000" | bc)
MIN_MS=$(echo "scale=2; $MIN/1000" | bc)
MAX_MS=$(echo "scale=2; $MAX/1000" | bc)
THROUGHPUT=$((1000000 * NUM_ORDERS / SUM))

# Calculate P50, P95, P99
SORTED=$(echo $LATENCIES | tr ' ' '\n' | sort -n)
P50_IDX=$((NUM_ORDERS * 50 / 100))
P95_IDX=$((NUM_ORDERS * 95 / 100))
P99_IDX=$((NUM_ORDERS * 99 / 100))

P50=$(echo $SORTED | cut -d' ' -f$P50_IDX)
P95=$(echo $SORTED | cut -d' ' -f$P95_IDX)
P99=$(echo $SORTED | cut -d' ' -f$P99_IDX)

P50_MS=$(echo "scale=2; $P50/1000" | bc)
P95_MS=$(echo "scale=2; $P95/1000" | bc)
P99_MS=$(echo "scale=2; $P99/1000" | bc)

echo "=========================================="
echo "  Results"
echo "=========================================="
echo ""
echo -e "${CYAN}Latency (client-side, includes network):${NC}"
echo "  Avg:   ${AVG}µs (${AVG_MS}ms)"
echo "  Min:   ${MIN}µs (${MIN_MS}ms)"
echo "  Max:   ${MAX}µs (${MAX_MS}ms)"
echo ""
echo -e "${CYAN}Percentiles:${NC}"
echo "  P50:   ${P50}µs (${P50_MS}ms)"
echo "  P95:   ${P95}µs (${P95_MS}ms)"
echo "  P99:   ${P99}µs (${P99_MS}ms)"
echo ""
echo -e "${CYAN}Throughput:${NC}"
echo "  ~${THROUGHPUT} orders/sec (sequential)"
echo ""
echo "=========================================="
echo ""
echo "Note: Server-side latency breakdown in logs:"
echo "  RUST_LOG=info ./target/release/order_gate_server"
echo "  Look for: [LATENCY] convert=Xµs validate+wal=Xµs kafka=Xµs"
