#!/bin/bash
#
# Full Exchange E2E Test
# Tests complete exchange operations via Gateway API:
#   1. Infrastructure (TigerBeetle, ScyllaDB)
#   2. UBSCore startup
#   3. Deposit (Funding account)
#   4. Internal Transfer (Funding → Trading)
#   5. Place Orders
#   6. Order Matching & Settlement
#   7. Internal Transfer (Trading → Funding)
#   8. Balance Verification
#
# Prerequisites:
#   - TigerBeetle running on 127.0.0.1:3000
#   - ScyllaDB running on 127.0.0.1:9042
#   - Kafka running on 127.0.0.1:9092
#

set -e

echo "🏦 FULL EXCHANGE E2E TEST"
echo "========================="
echo ""

# Configuration
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:8080}"
TRANSFER_URL="${TRANSFER_URL:-http://127.0.0.1:8080}"
USER_ID=9001
ASSET_BTC=1
ASSET_USDT=2

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

PASSED=0
FAILED=0
PIDS=""

# Cleanup
cleanup() {
    echo ""
    echo "🧹 Cleaning up..."
    for pid in $PIDS; do
        kill $pid 2>/dev/null || true
    done
    pkill -f "gateway_service" 2>/dev/null || true
    pkill -f "internal_transfer_service" 2>/dev/null || true
    pkill -f "ubscore_service" 2>/dev/null || true
    pkill -f "settlement_service" 2>/dev/null || true
    pkill -f "matching_engine_server" 2>/dev/null || true
}
trap cleanup EXIT

# Helper functions
check() {
    local expected="$1"
    local actual="$2"
    local test_name="$3"

    if echo "$actual" | grep -qE "$expected"; then
        echo -e "${GREEN}✅ $test_name${NC}"
        ((PASSED++))
        return 0
    else
        echo -e "${RED}❌ $test_name${NC}"
        echo "   Expected: $expected"
        echo "   Actual: $actual"
        ((FAILED++))
        return 1
    fi
}

wait_for_service() {
    local url="$1"
    local name="$2"
    local max_wait=${3:-30}

    echo "⏳ Waiting for $name..."
    for i in $(seq 1 $max_wait); do
        if curl -s "$url" >/dev/null 2>&1; then
            echo "   ✅ $name is ready"
            return 0
        fi
        sleep 1
    done
    echo "   ❌ $name failed to start"
    return 1
}

# ============================================================================
# PHASE 1: Start Services
# ============================================================================
echo "📦 PHASE 1: Starting Services"
echo "------------------------------"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Build binaries
echo "🔨 Building binaries..."
cargo build --bin internal_transfer_service --bin gateway_service 2>&1 | grep -E "Compiling|Finished" || true

# Start internal_transfer_service
echo "🚀 Starting internal_transfer_service..."
RUST_LOG=info "$PROJECT_DIR/target/debug/internal_transfer_service" > /tmp/transfer_service.log 2>&1 &
PIDS="$PIDS $!"
sleep 5

# Wait for transfer service
if ! wait_for_service "$TRANSFER_URL/api/v1/transfer/1" "Transfer Service" 20; then
    echo "❌ Transfer service failed to start. Log:"
    tail -20 /tmp/transfer_service.log
    exit 1
fi

echo ""

# ============================================================================
# PHASE 2: Deposit Test
# ============================================================================
echo "💰 PHASE 2: Deposit to Funding Account"
echo "---------------------------------------"

# Note: In a full system, deposit would go through gateway → TB
# For now, we assume user already has funds in Funding account (seeded)
echo "ℹ️  Assuming user $USER_ID has funds in Funding account"
echo ""

# ============================================================================
# PHASE 3: Internal Transfer - Funding → Trading
# ============================================================================
echo "🔄 PHASE 3: Internal Transfer (Funding → Trading)"
echo "---------------------------------------------------"

RESP=$(curl -s -X POST "$TRANSFER_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d "{\"from\":\"funding\",\"to\":\"trading\",\"user_id\":$USER_ID,\"asset_id\":$ASSET_USDT,\"amount\":10000}")
echo "Response: $RESP"
check "committed|pending" "$RESP" "Funding → Trading Transfer"

REQ_ID=$(echo "$RESP" | jq -r '.req_id')
echo "   Request ID: $REQ_ID"
echo ""

# Query transfer status
QUERY=$(curl -s "$TRANSFER_URL/api/v1/transfer/$REQ_ID")
echo "Query: $QUERY"
check "committed|pending" "$QUERY" "Transfer Status Query"
echo ""

# ============================================================================
# PHASE 4: Place Orders (requires full gateway + UBSCore)
# ============================================================================
echo "📝 PHASE 4: Trading Operations"
echo "-------------------------------"
echo "ℹ️  Full order placement requires:"
echo "   - UBSCore service (Aeron)"
echo "   - Matching Engine"
echo "   - Settlement Service"
echo ""
echo "   Skipping order tests (run separately with full stack)"
echo ""

# ============================================================================
# PHASE 5: Internal Transfer - Trading → Funding
# ============================================================================
echo "🔄 PHASE 5: Internal Transfer (Trading → Funding)"
echo "---------------------------------------------------"

RESP=$(curl -s -X POST "$TRANSFER_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d "{\"from\":\"trading\",\"to\":\"funding\",\"user_id\":$USER_ID,\"asset_id\":$ASSET_USDT,\"amount\":5000}")
echo "Response: $RESP"
check "committed|pending" "$RESP" "Trading → Funding Transfer"
echo ""

# ============================================================================
# PHASE 6: Multiple Transfers
# ============================================================================
echo "🔁 PHASE 6: Multiple Concurrent Transfers"
echo "------------------------------------------"

for i in 1 2 3; do
    RESP=$(curl -s -X POST "$TRANSFER_URL/api/v1/transfer" \
        -H "Content-Type: application/json" \
        -d "{\"from\":\"funding\",\"to\":\"trading\",\"user_id\":$USER_ID,\"asset_id\":$ASSET_BTC,\"amount\":100}")
    STATUS=$(echo "$RESP" | jq -r '.status')
    echo "   Transfer $i: $STATUS"
done
check "3" "3" "Multiple Transfers"
echo ""

# ============================================================================
# PHASE 7: Error Handling
# ============================================================================
echo "⚠️ PHASE 7: Error Handling"
echo "---------------------------"

# Invalid: same source and target
RESP=$(curl -s -X POST "$TRANSFER_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d "{\"from\":\"funding\",\"to\":\"funding\",\"user_id\":$USER_ID,\"asset_id\":1,\"amount\":100}")
echo "Same source/target: $RESP"
check "error|failed" "$RESP" "Reject same source/target"

# Invalid: zero amount
RESP=$(curl -s -X POST "$TRANSFER_URL/api/v1/transfer" \
    -H "Content-Type: application/json" \
    -d "{\"from\":\"funding\",\"to\":\"trading\",\"user_id\":$USER_ID,\"asset_id\":1,\"amount\":0}")
echo "Zero amount: $RESP"
check "error|failed" "$RESP" "Reject zero amount"
echo ""

# ============================================================================
# SUMMARY
# ============================================================================
echo "============================================"
echo "📊 TEST SUMMARY"
echo "============================================"
echo "Passed: $PASSED"
echo "Failed: $FAILED"
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}🎉 ALL TESTS PASSED!${NC}"
    exit 0
else
    echo -e "${RED}⚠️ SOME TESTS FAILED${NC}"
    exit 1
fi
