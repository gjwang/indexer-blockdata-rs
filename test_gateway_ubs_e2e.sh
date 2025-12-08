#!/bin/bash
#
# E2E Test: Order flow from Gateway API to UBSCore
#
# Flow: HTTP POST → Gateway → Kafka(orders) → UBSCore → Kafka(validated_orders)
#

set -e

echo "=========================================="
echo "  E2E Test: Gateway → UBSCore"
echo "=========================================="

# Configuration
GATEWAY_URL="http://localhost:3001"
KAFKA_BROKER="localhost:9092"
ORDERS_TOPIC="orders"
VALIDATED_TOPIC="validated_orders"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if services are running
check_service() {
    local name=$1
    local port=$2
    if nc -z localhost $port 2>/dev/null; then
        echo -e "${GREEN}✓${NC} $name is running on port $port"
        return 0
    else
        echo -e "${RED}✗${NC} $name is NOT running on port $port"
        return 1
    fi
}

check_process() {
    local name=$1
    local pattern=$2
    if pgrep -f "$pattern" > /dev/null; then
        echo -e "${GREEN}✓${NC} $name is running"
        return 0
    else
        echo -e "${RED}✗${NC} $name is NOT running"
        return 1
    fi
}

echo ""
echo "Step 1: Checking services..."
echo ""

SERVICES_OK=true

# Check Gateway
if ! check_service "Gateway API" 3001; then
    echo "  Start with: cargo run --bin order_gate_server"
    SERVICES_OK=false
fi

# Check UBSCore (process, not port - it's a Kafka consumer)
if ! check_process "UBSCore" "ubscore_service"; then
    echo "  Start with: cargo run --bin ubscore_service"
    SERVICES_OK=false
fi

# Check Kafka
if ! check_service "Kafka/Redpanda" 9092; then
    echo "  Start with: docker-compose up -d redpanda"
    SERVICES_OK=false
fi

if [ "$SERVICES_OK" = false ]; then
    echo ""
    echo -e "${YELLOW}Please start missing services and re-run this test.${NC}"
    exit 1
fi

echo ""
echo "Step 2: Seeding account with Transfer In..."
echo ""

USER_ID=1001

# Transfer In USDT for testing
TRANSFER_RESPONSE=$(curl -s -X POST "$GATEWAY_URL/api/v1/transfer_in" \
    -H "Content-Type: application/json" \
    -d "{
        \"request_id\": \"seed_$(date +%s)\",
        \"user_id\": $USER_ID,
        \"asset\": \"USDT\",
        \"amount\": \"100000.00\"
    }" 2>&1)

echo "Transfer In Response:"
echo "$TRANSFER_RESPONSE" | jq . 2>/dev/null || echo "$TRANSFER_RESPONSE"
echo ""

if echo "$TRANSFER_RESPONSE" | grep -q "success.*true"; then
    echo -e "${GREEN}✓${NC} Transfer In successful"
else
    echo -e "${YELLOW}⚠${NC} Transfer In may have failed (check log)"
fi

echo ""
echo "Step 3: Sending test order via Gateway API..."
echo ""

# Send a test order
ORDER_RESPONSE=$(curl -s -X POST "$GATEWAY_URL/api/orders?user_id=$USER_ID" \
    -H "Content-Type: application/json" \
    -d '{
        "symbol": "BTC_USDT",
        "side": "Buy",
        "price": "50000.00",
        "quantity": "1.5",
        "order_type": "Limit"
    }' 2>&1)

echo "Gateway Response:"
echo "$ORDER_RESPONSE" | jq . 2>/dev/null || echo "$ORDER_RESPONSE"
echo ""

# Check if order was accepted
if echo "$ORDER_RESPONSE" | grep -q "order_id\|status.*0"; then
    echo -e "${GREEN}✓${NC} Order submitted to Gateway"
else
    echo -e "${RED}✗${NC} Order submission failed"
    echo "$ORDER_RESPONSE"
fi

echo ""
echo "Step 4: Checking Kafka topics..."
echo ""

# Check orders topic (requires rpk or kafka-console-consumer)
if command -v rpk &> /dev/null; then
    echo "Orders topic (last 3 messages):"
    rpk topic consume $ORDERS_TOPIC --brokers $KAFKA_BROKER -n 3 --offset end 2>/dev/null || echo "  (no messages or topic doesn't exist)"
    echo ""

    echo "Validated orders topic (last 3 messages):"
    rpk topic consume $VALIDATED_TOPIC --brokers $KAFKA_BROKER -n 3 --offset end 2>/dev/null || echo "  (no messages or topic doesn't exist)"
else
    echo -e "${YELLOW}Note: Install 'rpk' (Redpanda CLI) to inspect Kafka topics${NC}"
fi

echo ""
echo "Step 5: Checking UBSCore logs..."
echo ""

if [ -f "logs/ubscore.log" ]; then
    echo "Last 10 lines of ubscore.log:"
    tail -10 logs/ubscore.log
else
    echo "(No log file found at logs/ubscore.log)"
fi

echo ""
echo "=========================================="
echo "  Test Complete"
echo "=========================================="
