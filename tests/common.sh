#!/bin/bash
# Common test helpers

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

setup_env() {
    echo "🧹 Cleaning environment..."
    docker rm -f tigerbeetle || true
    pkill -f ubscore_service || true
    rm -rf data/tigerbeetle/*
    mkdir -p data/tigerbeetle
    mkdir -p triggers
    rm -f triggers/*

    docker-compose down -v
    docker-compose up -d --remove-orphans
    sleep 5

    echo "🐯 Setting up TigerBeetle..."
    docker run --privileged --rm -v $(pwd)/data/tigerbeetle:/data ghcr.io/tigerbeetle/tigerbeetle:latest format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle > /dev/null
    docker run --privileged -d --name tigerbeetle -p 3000:3000 -v $(pwd)/data/tigerbeetle:/data ghcr.io/tigerbeetle/tigerbeetle:latest start --addresses=0.0.0.0:3000 /data/0_0.tigerbeetle > /dev/null

    echo "▶️  Running UBSCore Service..."
    cargo run --bin ubscore_service > service.log 2>&1 &
    PID=$!

    # Wait for startup
    echo -n "⏳ Waiting for Service..."
    local TIMEOUT=30
    local COUNT=0
    while [ $COUNT -lt $TIMEOUT ]; do
        if grep -q "Test Command Mode initialized" service.log; then
            echo -e " ${GREEN}READY${NC}"
            return 0
        fi
        sleep 1
        let COUNT=COUNT+1
        echo -n "."
    done

    echo -e " ${RED}TIMEOUT${NC}"
    tail -n 10 service.log
    kill $PID
    exit 1
}

send_cmd() {
    echo "👉 Sending: $*"
    echo "$*" > triggers/command
    # Wait for file to be consumed (deleted)
    while [ -f triggers/command ]; do
        sleep 0.1
    done
}

check_log() {
    local PATTERN=$1
    if grep -q "$PATTERN" service.log; then
        echo -e "✅ Found: $PATTERN"
    else
        echo -e "❌ MISSING: $PATTERN"
        echo "Tail:"
        tail -n 5 service.log
        kill $PID
        exit 1
    fi
}

cleanup() {
    send_cmd "EXIT"
    kill $PID || true
}
