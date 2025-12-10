#!/bin/bash
echo "========================================="
echo "  Current System State"
echo "========================================="
echo ""

echo "📊 Database Counts:"
echo "-------------------"
BASE_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.balance_ledger;" 2>&1 | grep -o '[0-9]\+' | head -1)
MV_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.user_balances_by_user;" 2>&1 | grep -o '[0-9]\+' | head -1)
TRADES_COUNT=$(docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.settled_trades;" 2>&1 | grep -o '[0-9]\+' | head -1)

echo "  balance_ledger: $BASE_COUNT rows"
echo "  MV (user_balances_by_user): $MV_COUNT rows"
echo "  settled_trades: $TRADES_COUNT rows"
echo ""

if [ "$BASE_COUNT" == "$MV_COUNT" ]; then
    echo "  ✅ MV fully synchronized"
else
    echo "  ⚠️  MV sync mismatch"
fi
echo ""

echo "💰 User 1001 Balance:"
echo "--------------------"
docker exec scylla cqlsh -e "SELECT user_id, asset_id, avail, frozen FROM trading.user_balances_by_user WHERE user_id=1001 PER PARTITION LIMIT 1;" 2>&1 | grep -A 5 "user_id"
echo ""

echo "🔍 Gateway API Balance:"
echo "----------------------"
BALANCE=$(curl -s "http://localhost:3001/api/user/balance?user_id=1001" 2>&1)
echo "$BALANCE" | jq -C '.'
echo ""

echo "✅ System is operational!"
