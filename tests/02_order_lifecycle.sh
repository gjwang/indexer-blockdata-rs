#!/bin/bash
source tests/common.sh

echo "🧪 TEST 02: Order Lifecycle (Lock/Unlock)"
setup_env

# 1. Deposit
send_cmd "DEPOSIT 2001 2 100000 888001"
# 2. Lock 500
send_cmd "LOCK 2001 2 500 9001"
sleep 1
check_log "Funds Locked: user=2001 amount=500"

# 3. Unlock 500 (Cancel)
send_cmd "UNLOCK 2001 2 500 9001"
sleep 1
check_log "Funds Unlocked: user=2001"

cleanup
echo "🎉 TEST 02 PASSED"
