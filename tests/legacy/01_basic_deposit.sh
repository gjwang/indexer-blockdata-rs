#!/bin/bash
source tests/common.sh

echo "🧪 TEST 01: Basic Deposit"
setup_env

# Deposit 100 BTC (Asset 1) for User 1001
send_cmd "DEPOSIT 1001 1 10000000000 777001"
sleep 1
check_log "Deposited: user=1001 asset=1 amount=10000000000"

# Deposit 5000 USDT (Asset 2) for User 1001
send_cmd "DEPOSIT 1001 2 50000000000 777002"
sleep 1
check_log "Deposited: user=1001 asset=2 amount=50000000000"

cleanup
echo "🎉 TEST 01 PASSED"
