#!/bin/bash
set -e

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "🚀 Starting Step-by-Step Verification..."

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
    local STEP=$1
    check_log_wait "Ready for '$STEP'" "Driver Ready ($STEP)"
}

trigger_step() {
    local STEP=$1
    echo "👉 Triggering $STEP..."
    touch triggers/$STEP
}

# --- TEST FLOW ---

# Wait for Service Startup and Step 1 Driver Ready
wait_for_driver_ready "step1"

# STEP 1: DEPOSIT
echo -e "\n${YELLOW}--- STEP 1: DEPOSIT ---${NC}"
trigger_step "step1"
check_log_wait "AccountCreated" "Event: AccountCreated"
check_log_wait "Deposited" "Event: Deposited"
check_log_wait "STEP 1] Finished" "Step 1 Completion"
echo "✅ Step 1 Verified."

# STEP 2: LOCK FUNDS
wait_for_driver_ready "step2"
echo -e "\n${YELLOW}--- STEP 2: LOCK FUNDS ---${NC}"
trigger_step "step2"
check_log_wait "FundsLocked" "Event: FundsLocked"
check_log_wait "STEP 2] Success" "Step 2 Completion"
echo "✅ Step 2 Verified."

# STEP 3: SETTLE TRADE
wait_for_driver_ready "step3"
echo -e "\n${YELLOW}--- STEP 3: SETTLE TRADE ---${NC}"
trigger_step "step3"
check_log_wait "TradeSettled" "Event: TradeSettled"
check_log_wait "STEP 3] Success" "Step 3 Completion"
echo "✅ Step 3 Verified."


echo -e "\n${GREEN}🎉 ALL STEPS PASSED.${NC}"
kill $PID
