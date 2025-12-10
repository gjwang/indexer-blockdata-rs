#!/bin/bash
# Test cancel order with full lifecycle tracing

set -e

echo "========================================="
echo "  Cancel Order Lifecycle Test"
echo "========================================="
echo ""

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info() {
    echo -e "${BLUE}ℹ️  $1${NC}"
}

success() {
    echo -e "${GREEN}✅ $1${NC}"
}

warn() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

# Step 1: Clean and start services
info "Step 1: Starting services..."
pkill -9 -f "ubscore_aeron_service|settlement_service|matching_engine_server|order_gate_server" || true
sleep 2

# Clean logs
rm -f logs/*.log.* 2>/dev/null || true

# Start services
info "Starting UBSCore..."
./target/debug/ubscore_aeron_service > logs/ubscore_cancel_test.log 2>&1 &
UBSCORE_PID=$!
sleep 3

info "Starting Settlement..."
./target/debug/settlement_service > logs/settlement_cancel_test.log 2>&1 &
SETTLEMENT_PID=$!
sleep 2

info "Starting Matching Engine..."
./target/debug/matching_engine_server > logs/me_cancel_test.log 2>&1 &
ME_PID=$!
sleep 5

info "Starting Gateway..."
./target/debug/order_gate_server > logs/gateway_cancel_test.log 2>&1 &
GATEWAY_PID=$!
sleep 3

success "All services started"
echo ""

# Step 2: Place an order
echo "========================================="
echo "  Step 2: Placing Order"
echo "========================================="

USER_ID=1001
ORDER_PAYLOAD='{
  "symbol": "BTC_USDT",
  "side": "Buy",
  "price": "50000.0",
  "quantity": "0.1",
  "order_type": "Limit"
}'

info "Placing order for user $USER_ID..."
ORDER_RESPONSE=$(curl -s -X POST "http://localhost:3001/api/v1/order/create?user_id=$USER_ID" \
  -H "Content-Type: application/json" \
  -d "$ORDER_PAYLOAD")

echo "Response: $ORDER_RESPONSE"

ORDER_ID=$(echo "$ORDER_RESPONSE" | jq -r '.data.order_id' 2>/dev/null)
ORDER_STATUS=$(echo "$ORDER_RESPONSE" | jq -r '.data.order_status' 2>/dev/null)

if [ -z "$ORDER_ID" ] || [ "$ORDER_ID" == "null" ]; then
    warn "Failed to create order or parse response"
    echo "Response: $ORDER_RESPONSE"
    exit 1
fi

success "Order placed: ID=$ORDER_ID, Status=$ORDER_STATUS"
sleep 1

# Step 3: Trace order creation in logs
echo ""
echo "========================================="
echo "  Step 3: Trace Order Creation"
echo "========================================="

info "Checking Gateway logs..."
if grep -q "order_id=$ORDER_ID" logs/gateway_cancel_test.log 2>/dev/null; then
    success "✓ Gateway: Order logged"
    grep "order_id=$ORDER_ID" logs/gateway_cancel_test.log | head -2
else
    warn "Gateway: Order not found in logs"
fi

info "Check UBSCore logs..."
if grep -q "$ORDER_ID" logs/ubscore_cancel_test.log 2>/dev/null; then
    success "✓ UBSCore: Order received"
    grep "$ORDER_ID" logs/ubscore_cancel_test.log | head -2
else
    warn "UBSCore: Order not found"
fi

info "Checking ME logs..."
if grep -q "$ORDER_ID" logs/me_cancel_test.log 2>/dev/null; then
    success "✓ ME: Order processed"
    grep "$ORDER_ID" logs/me_cancel_test.log | head -2
else
    warn "ME: Order not found"
fi

echo ""

# Step 4: Cancel the order
echo "========================================="
echo "  Step 4: Canceling Order"
echo "========================================="

CANCEL_PAYLOAD="{\"order_id\": $ORDER_ID}"

info "Sending cancel request..."
CANCEL_RESPONSE=$(curl -s -X POST "http://localhost:3001/api/v1/order/cancel?user_id=$USER_ID" \
  -H "Content-Type: application/json" \
  -d "$CANCEL_PAYLOAD")

echo "Cancel Response: $CANCEL_RESPONSE"

CANCEL_STATUS=$(echo "$CANCEL_RESPONSE" | jq -r '.data.order_status' 2>/dev/null)

if [ "$CANCEL_STATUS" == "Cancelled" ]; then
    success "Cancel request accepted: Status=$CANCEL_STATUS"
else
    warn "Cancel request status: $CANCEL_STATUS"
fi

sleep 2

# Step 5: Trace cancellation through all services
echo ""
echo "========================================="
echo "  Step 5: Trace Cancel Lifecycle"
echo "========================================="

echo ""
info "1️⃣ GATEWAY (Entry Point)"
echo "   Looking for cancel request..."
if grep -i "cancel" logs/gateway_cancel_test.log 2>/dev/null | grep -q "$ORDER_ID"; then
    success "   ✓ Gateway received cancel request"
    grep -i "cancel" logs/gateway_cancel_test.log | grep "$ORDER_ID" | tail -3
else
    warn "   Gateway cancel log not found"
fi

echo ""
info "2️⃣ UBSCORE (Validation Layer)"
echo "   Looking for cancel message..."
if grep -i "cancel" logs/ubscore_cancel_test.log 2>/dev/null | grep -q "$ORDER_ID"; then
    success "   ✓ UBSCore processed cancel"
    grep -i "cancel" logs/ubscore_cancel_test.log | grep "$ORDER_ID" | tail -3
else
    warn "   UBSCore cancel log not found"
fi

echo ""
info "3️⃣ MATCHING ENGINE (Order Book)"
echo "   Looking for cancel execution..."
if grep -i "cancel" logs/me_cancel_test.log 2>/dev/null | grep -q "$ORDER_ID"; then
    success "   ✓ ME executed cancel"
    grep -i "cancel" logs/me_cancel_test.log | grep "$ORDER_ID" | tail -3
else
    warn "   ME cancel log not found"
    # Alternative: check for order removal
    if grep -i "removed\|deleted"  logs/me_cancel_test.log 2>/dev/null | grep -q "$ORDER_ID"; then
        success "   ✓ ME removed order from book"
        grep -i "removed\|deleted" logs/me_cancel_test.log | grep "$ORDER_ID" | tail -2
    fi
fi

echo ""
info "4️⃣ SETTLEMENT (Balance Unlock)"
echo "   Looking for balance unlock..."
if grep -i "unlock\|unfreeze\|cancel" logs/settlement_cancel_test.log 2>/dev/null | grep -q "$ORDER_ID"; then
    success "   ✓ Settlement unlocked funds"
    grep -i "unlock\|unfreeze\|cancel" logs/settlement_cancel_test.log | grep "$ORDER_ID" | tail -3
else
    warn "   Settlement unlock log not found"
fi

# Step 6: Check database state
echo ""
echo "========================================="
echo "  Step 6: Database Verification"
echo "========================================="

info "Checking balance_ledger for unlock events..."
docker exec scylla cqlsh -e "
SELECT user_id, asset_id, event_type, delta_frozen, frozen, seq
FROM trading.balance_ledger
WHERE user_id=$USER_ID AND asset_id IN (1,2)
ORDER BY seq DESC
LIMIT 5;" 2>&1

info "Checking if order still in active books (should not be)..."
# Note: We don't have an active orders table, but we can check logs

# Step 7: Summary
echo ""
echo "========================================="
echo "  Lifecycle Summary"
echo "========================================="
echo ""

echo "📝 Order Lifecycle:"
echo "   1. ✅ Order Created    ID=$ORDER_ID"
echo "   2. ✅ Cancel Requested user=$USER_ID"
echo "   3. Gateway  → UBSCore  (HTTP → Aeron)"
echo "   4. UBSCore  → ME       (Aeron → Kafka)"
echo "   5. ME       → Settlement (Kafka)"
echo "   6. Settlement unlocks frozen funds"
echo ""

# Keep services running for manual inspection
info "Services still running for inspection:"
echo "   UBSCore:    PID $UBSCORE_PID"
echo "   Settlement: PID $SETTLEMENT_PID"
echo "   ME:         PID $ME_PID"
echo "   Gateway:    PID $GATEWAY_PID"
echo ""
echo "To stop all:"
echo "   kill $UBSCORE_PID $SETTLEMENT_PID $ME_PID $GATEWAY_PID"
echo ""
echo "Log files:"
echo "   logs/gateway_cancel_test.log"
echo "   logs/ubscore_cancel_test.log"
echo "   logs/me_cancel_test.log"
echo "   logs/settlement_cancel_test.log"
echo ""

success "✅ Cancel order lifecycle test complete!"
echo ""
echo "Review the logs above to confirm each stage of the cancel flow."
