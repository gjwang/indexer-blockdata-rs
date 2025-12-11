#!/bin/bash
source tests/common.sh

echo "🧪 TEST 04: Full Exchange Simulation (Partial Fills)"
setup_env

# Users: 4001 (Buyer), 4002 (Seller)
# Asset 1 (BTC), 2 (USDT)

echo "--- Phase 1: Deposits ---"
send_cmd "DEPOSIT 4001 2 100000 101" # Buyer: 100k USDT
send_cmd "DEPOSIT 4002 1 500000 102" # Seller: 5 BTC (Satoshis? Low scaling for simplicity)
sleep 1

echo "--- Phase 2: Locks ---"
# Buyer wants 2 BTC at 100 USDT/BTC -> Locks 200 USDT
send_cmd "LOCK 4001 2 200 9001"
# Seller wants to sell 5 BTC at 100 -> Locks 500 Satoshis? (Assuming 1=1 for simplicity here)
send_cmd "LOCK 4002 1 500 9002"
sleep 1

echo "--- Phase 3: Partial Match 1 ---"
# Match 1 BTC: Price 100.
# Buyer 4001 buys 1 from Seller 4002.
# SETTLE match_id=8001
send_cmd "SETTLE 8001 4001 4002 1 2 100 1 9001 9002"
sleep 1
check_log "Trade Settled: match_id=8001"

echo "--- Phase 4: Partial Match 2 ---"
# Match another 1 BTC: Price 100.
# Buyer 4001 buys 1 from Seller 4002.
# SETTLE match_id=8002
send_cmd "SETTLE 8002 4001 4002 1 2 100 1 9001 9002"
sleep 1
check_log "Trade Settled: match_id=8002"

echo "--- Phase 5: Remainder ---"
# Buyer Order 9001 is filled (200 locked, 200 spent).
# Seller Order 9002 has 300 remaining locked.
# Seller Cancels remaining.
send_cmd "UNLOCK 4002 1 300 9002"
sleep 1
check_log "Funds Unlocked: user=4002"

cleanup
echo "🎉 TEST 04 PASSED"
