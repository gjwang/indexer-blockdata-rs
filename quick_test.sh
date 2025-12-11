#!/bin/bash
# Quick Internal Transfer Test

echo "🧪 Testing Internal Transfer API"

# Test data
curl -X POST http://localhost:3001/api/v1/user/internal_transfer \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {"account_type": "funding", "asset": "USDT"},
    "to_account": {"account_type": "spot", "user_id": 3001, "asset": "USDT"},
    "amount": "100.00000000"
  }'

echo ""
echo "✅ Test complete"
