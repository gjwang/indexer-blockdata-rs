#!/bin/bash

BASE_URL="http://localhost:3001"

echo "Testing Gateway API..."

USER_ID=123

echo "1. Get Balance"
curl -s "$BASE_URL/api/user/balance?user_id=$USER_ID"
echo ""

echo "2. Get Trade History (BTC_USDT)"
curl -s "$BASE_URL/api/user/trade_history?user_id=$USER_ID&symbol=BTC_USDT"
echo ""

echo "3. Get Order History (BTC_USDT)"
curl -s "$BASE_URL/api/user/order_history?user_id=$USER_ID&symbol=BTC_USDT"
echo ""

echo "Done."
