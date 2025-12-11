#!/bin/bash
source tests/common.sh

echo "🧪 TEST 03: Trade Settlement (End-to-End)"
setup_env

# 1. Deposit Buyer (USDT) & Seller (BTC)
send_cmd "DEPOSIT 3001 2 55000000000 1001" # Buyer: 55k USDT
send_cmd "DEPOSIT 3002 1 2000000000 1002"  # Seller: 2 BTC

# 2. Lock Funds
# Buyer locks 50k USDT for Bid
send_cmd "LOCK 3001 2 50000000000 9001"
sleep 1
check_log "Funds Locked: user=3001"

# Seller locks 1 BTC for Ask (simulated by script or implict in settle?)
# The UBSCore settle logic expects Seller funds to be locked.
send_cmd "LOCK 3002 1 1000000000 9002"
sleep 1
check_log "Funds Locked: user=3002"

# 3. Settle
# SETTLE match_id buy_uid sell_uid base quote price qty buy_oid sell_oid
# Price=50000 USDT, Qty=1 BTC
send_cmd "SETTLE 8001 3001 3002 1 2 50000 1000000000 9001 9002"
sleep 1
check_log "Trade Settled: match_id=8001"

cleanup
echo "🎉 TEST 03 PASSED"
