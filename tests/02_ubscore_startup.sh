#!/bin/bash
set -e

# Test 02: UBSCore Service Startup
# Verifies UBSCore starts correctly and initializes TigerBeetle accounts

echo "🧪 TEST 02: UBSCore Service Startup"
echo "======================================"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

# Run infrastructure test first
echo "📋 Running infrastructure setup..."
bash tests/01_infrastructure.sh || exit 1

# Cleanup function
cleanup() {
    echo "🧹 Cleanup..."
    pkill -f ubscore_aeron_service || true
}
trap cleanup EXIT

# Build UBSCore
echo "🔨 Building UBSCore service..."
cargo build --bin ubscore_aeron_service --features aeron --quiet

# Start UBSCore
echo "▶️  Starting UBSCore Service..."
rm -rf ~/ubscore_data logs/*
SEED_TEST_ACCOUNTS=0 ./target/debug/ubscore_aeron_service --features aeron > logs/ubscore_std.log 2>&1 &
UBS_PID=$!

# Wait for UBSCore to  be ready
DATE=$(date +%Y-%m-%d)
LOG_FILE="logs/ubscore.log.$DATE"

echo -n "⏳ Waiting for UBSCore..."
sleep 5
timeout=30
count=0
while [ $count -lt $timeout ]; do
    if [ -f "$LOG_FILE" ] && grep -q "UBSCore Service ready" "$LOG_FILE"; then
        echo -e " ${GREEN}READY${NC}"
        break
    fi
    if ! kill -0 $UBS_PID 2>/dev/null; then
        echo -e " ${RED}UBSCore died!${NC}"
        cat logs/ubscore_std.log
        exit 1
    fi
    sleep 1
    let count=count+1
    echo -n "."
done

if [ $count -ge $timeout ]; then
    echo-e " ${RED}TIMEOUT${NC}"
    tail -n 20 "$LOG_FILE" 2>/dev/null || cat logs/ubscore_std.log
    exit 1
fi

# Verify TigerBeetle initialization
echo "🐯 Verifying TigerBeetle initialization..."
if grep -q "System accounts initialized" "$LOG_FILE"; then
    echo -e " ${GREEN}✅ TigerBeetle accounts initialized${NC}"
else
    echo -e " ${RED}❌ TigerBeetle initialization failed${NC}"
    tail -n 20 "$LOG_FILE"
    exit 1
fi

# Verify event loop started
if grep -q "Starting event loop" "$LOG_FILE"; then
    echo -e " ${GREEN}✅ Event loop started${NC}"
else
    echo -e " ${RED}❌ Event loop not started${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}🎉 TEST 02 PASSED - UBSCore Running${NC}"
exit 0
