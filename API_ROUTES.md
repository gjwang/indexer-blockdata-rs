# API Routes - Final Structure

**Version:** v1
**Date:** 2025-12-10
**Status:** ✅ Production Ready

---

## 📋 API Endpoints

### **Order Operations**
```
POST /api/v1/order/create      - Create new order
POST /api/v1/order/cancel      - Cancel existing order
```

### **User Operations**
```
GET  /api/v1/user/balance      - Get user balances (all assets)
GET  /api/v1/user/trades       - Get user trade history
GET  /api/v1/user/orders       - Get user order history
POST /api/v1/user/transfer_in  - Deposit funds
POST /api/v1/user/transfer_out - Withdraw funds
```

---

## 🎯 Design Principles

1. **Versioned**: All routes under `/api/v1/`
2. **Categorized**:
   - `/order/*` for order-related operations
   - `/user/*` for user-specific operations
3. **Concise**: Singular nouns (`order` not `orders`)
4. **Consistent**: Same pattern across all endpoints
5. **RESTful**: Proper HTTP methods (GET/POST)

---

## 📝 Examples

### Create Order
```bash
curl -X POST "http://localhost:3001/api/v1/order/create?user_id=1001" \
  -H "Content-Type: application/json" \
  -d '{
    "symbol": "BTC_USDT",
    "side": "Buy",
    "price": "50000.0",
    "quantity": "0.1",
    "order_type": "Limit"
  }'
```

### Cancel Order
```bash
curl -X POST "http://localhost:3001/api/v1/order/cancel?user_id=1001" \
  -H "Content-Type: application/json" \
  -d '{"order_id": 123456}'
```

### Get Balance
```bash
curl "http://localhost:3001/api/v1/user/balance?user_id=1001"
```

### Deposit
```bash
curl -X POST "http://localhost:3001/api/v1/user/transfer_in" \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 1001,
    "asset": "BTC",
    "amount": "1000.0",
    "request_id": "deposit_123"
  }'
```

### Withdraw
```bash
curl -X POST "http://localhost:3001/api/v1/user/transfer_out" \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": 1001,
    "asset": "BTC",
    "amount": "500.0",
    "request_id": "withdraw_456"
  }'
```

---

## ⚙️ Configuration

### UBSCore Timeout
Location: `src/bin/order_gate_server.rs`
```rust
ubscore_timeout_ms: 5000  // 5 seconds
```

This timeout is used for:
- Order validation via UBSCore
- Cancel order requests to UBSCore

**Configurable**: Adjust in `AppState` initialization

---

## ✅ Testing

All E2E tests updated and passing with new routes:
- ✅ Deposit test
- ✅ Create Order test
- ✅ Cancel Order test
- ✅ Withdraw test
- ✅ Balance verification

---

## 🔄 Migration Notes

### Changed Routes:

| Old Route | New Route |
|-----------|-----------|
| `/api/orders` | `/api/v1/order/create` |
| `/api/orders/cancel` | `/api/v1/order/cancel` |
| `/api/user/balance` | `/api/v1/user/balance` |
| `/api/user/trade_history` | `/api/v1/user/trades` |
| `/api/user/order_history` | `/api/v1/user/orders` |
| `/api/v1/transfer_in` | `/api/v1/user/transfer_in` |
| `/api/v1/transfer_out` | `/api/v1/user/transfer_out` |

### Breaking Changes:
- ✅ All routes now under `/api/v1/`
- ✅ User operations under `/user/` namespace
- ✅ Order operations under `/order/` namespace

---

## 📚 Architecture

```
Gateway API (/api/v1/)
├── order/
│   ├── create  → UBSCore → Kafka → ME
│   └── cancel  → UBSCore → Kafka → ME
└── user/
    ├── balance       → ScyllaDB (MV)
    ├── trades        → ScyllaDB
    ├── orders        → ScyllaDB
    ├── transfer_in   → UBSCore → Settlement
    └── transfer_out  → UBSCore → Settlement
```

---

## 🚀 Production Ready

- ✅ Clean, consistent API design
- ✅ No hardcoded timeouts
- ✅ Properly versioned
- ✅ All tests passing
- ✅ Documented and ready to use

---

*Last Updated: 2025-12-10 19:39 UTC+8*
