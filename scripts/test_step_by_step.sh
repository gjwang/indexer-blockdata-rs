#!/bin/bash
set -e

# Updated to use the new Command Processor architecture
# This script mimics the old "Step-by-Step" flow using the new "DEPOSIT / LOCK / SETTLE" commands.

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "🚀 Starting Step-by-Step Verification (Legacy Flow)..."

# 1. Clean Environment
echo "🧹 Cleaning environment..."
docker rm -f tigerbeetle || true
pkill -f ubscore_service || true
rm -rf data/tigerbeetle/*
mkdir -p data/tigerbeetle
mkdir -p triggers
rm -f triggers/*

docker-compose down -v
docker-compose up -d --remove-orphans

# 2. Check Redpanda/Scylla health
echo "⏳ Waiting for Redpanda/Scylla..."
sleep 5

# 3. Setup TigerBeetle
echo "🐯 Setting up TigerBeetle..."
docker run --privileged --rm -v $(pwd)/data/tigerbeetle:/data ghcr.io/tigerbeetle/tigerbeetle:latest format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle > /dev/null
docker run --privileged -d --name tigerbeetle -p 3000:3000 -v $(pwd)/data/tigerbeetle:/data ghcr.io/tigerbeetle/tigerbeetle:latest start --addresses=0.0.0.0:3000 /data/0_0.tigerbeetle > /dev/null

echo "✅ Environment Ready."

# 4. Run UBSCore Service
echo "▶️  Running UBSCore Service..."
cargo run --bin ubscore_service > service.log 2>&1 &
PID=$!

# Helper to check logs
check_log_wait() {
    local PATTERN=$1
    local STEP_NAME=$2
    local TIMEOUT=30
    local COUNT=0
    echo -n "👀 Waiting for '$STEP_NAME' confirmation..."
    while [ $COUNT -lt $TIMEOUT ]; do
        if grep -q "$PATTERN" service.log; then
            echo -e " ${GREEN}OK${NC}"
            return 0
        fi
        sleep 1
        let COUNT=COUNT+1
        echo -n "."
    done
    echo -e " ${RED}FAIL${NC}"
    echo "Tail of service.log:"
    tail -n 10 service.log
    kill $PID
    exit 1
}

wait_for_driver_ready() {
    # New driver doesn't log "Ready for stepX", it logs "Test Command Mode initialized" once.
    if ! grep -q "Test Command Mode initialized" service.log; then
         check_log_wait "Test Command Mode initialized" "Service Init"
    fi
}

send_cmd() {
    echo "👉 Triggering: $*"
    echo "$*" > triggers/command
    # Wait for consumption
    while [ -f triggers/command ]; do sleep 0.1; done
}

check_wait() {
    echo -n "Press Enter to execute $1..."
    # read  # Uncomment for true interactive mode
}

# --- TEST FLOW ---

wait_for_driver_ready

# STEP 1: DEPOSIT
# Old logic: Deposit User 2001 (10k BTC, 50k USDT) and 2002.
# New Command: DEPOSIT <user> <asset> <amount> <tx>
echo -e "\n${YELLOW}--- STEP 1: DEPOSIT ---${NC}"
send_cmd "DEPOSIT 2001 1 1000000000000 2002001"
send_cmd "DEPOSIT 2001 2 5000000000000 2002002"
send_cmd "DEPOSIT 2002 1 1000000000000 2002003"
check_log_wait "Deposited: user=2001" "Deposit 2001"
echo "✅ Step 1 Verified."

# STEP 2: LOCK FUNDS
echo -e "\n${YELLOW}--- STEP 2: LOCK FUNDS ---${NC}"
# Buyer 2001 locks 50k USDT (Asset 2)
send_cmd "LOCK 2001 2 50000000001000 9000001"
check_log_wait "Funds Locked: user=2001" "Lock Buyer"
echo "✅ Step 2 Verified."

# STEP 3: SETTLE TRADE
echo -e "\n${YELLOW}--- STEP 3: SETTLE TRADE ---${NC}"
# Pre-lock Seller 2002 (BTC)
send_cmd "LOCK 2002 1 1000000000 9000002"
# Settle
# SETTLE match_id buy sell base quote price qty ...
send_cmd "SETTLE 8000001 2001 2002 1 2 1000000000 50000000000 9000001 9000002"
check_log_wait "Trade Settled: match_id=8000001" "Settlement"
echo "✅ Step 3 Verified."


echo -e "\n${GREEN}🎉 ALL STEPS PASSED.${NC}"
kill $PID
