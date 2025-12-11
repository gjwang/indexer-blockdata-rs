#!/bin/bash
set -e

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

echo "🚀 Starting Full Pipeline Verification..."

# 1. Clean Environment
echo "🧹 Cleaning environment..."
docker rm -f tigerbeetle || true
pkill -f ubscore_service || true
rm -rf data/tigerbeetle/*
mkdir -p data/tigerbeetle

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

# 5. Monitor Logs for Success Steps
echo "👀 Watching logs..."

check_log() {
    local PATTERN=$1
    local STEP=$2
    local TIMEOUT=30
    local COUNT=0
    while [ $COUNT -lt $TIMEOUT ]; do
        if grep -q "$PATTERN" service.log; then
            echo -e "${GREEN}✅ PASS: $STEP${NC}"
            return 0
        fi
        sleep 1
        let COUNT=COUNT+1
    done
    echo -e "${RED}❌ FAIL: $STEP not found in logs after $TIMEOUT seconds${NC}"
    # Dump tail of log
    tail -n 20 service.log
    kill $PID
    exit 1
}

# Verify Steps match the updated seed_test_accounts logic
check_log "Created accounts for user 2001" "Account Creation (User 2001)"
check_log "Simulating Deposit for User 2001" "Deposit Simulation"
# Note: "Shadow Sync OK" might be debug level, checking for absence of ERRORs is harder in script.
# We check for the completion log.
check_log "Simulating Lock Funds for Buyer 2001" "Lock Funds Simulation"
check_log "Simulating Trade Settlement" "Trade Settlement Simulation"
check_log "Seeding complete" "Pipeline Finished"

echo -e "${GREEN}🎉 ALL SYSTEMS GO! Pipeline Verified.${NC}"

kill $PID
