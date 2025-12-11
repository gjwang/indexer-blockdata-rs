# Internal Transfer - Quick Start Guide

**Purpose:** Get the internal transfer feature up and running quickly

---

## Prerequisites

1. **ScyllaDB** running and accessible
2. **Rust toolchain** installed
3. **Project dependencies** installed (`cargo build`)

---

## Step 1: Setup Database

```bash
# Start ScyllaDB (if using Docker)
docker run --name scylla -d -p 9042:9042 scylladb/scylla

# Wait for startup
sleep 30

# Create schema
cqlsh -f schema/internal_transfer.cql
```

---

## Step 2: Build the Project

```bash
cargo build --lib
cargo test --lib
```

---

## Step 3: Use the API (Code Example)

```rust
use fetcher::api::InternalTransferHandler;
use fetcher::db::InternalTransferDb;
use fetcher::models::internal_transfer_types::{AccountType, InternalTransferRequest};
use fetcher::symbol_manager::SymbolManager;
use rust_decimal::Decimal;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Setup dependencies
    let db_session = /* get ScyllaDB session */;
    let db = Arc::new(InternalTransferDb::new(db_session));
    let symbol_manager = Arc::new(SymbolManager::load_from_db());

    // 2. Create handler
    let handler = InternalTransferHandler::new(db, symbol_manager);

    // 3. Create request
    let request = InternalTransferRequest {
        from_account: AccountType::Funding {
            asset: "USDT".to_string(),
        },
        to_account: AccountType::Spot {
            user_id: 3001,
            asset: "USDT".to_string(),
        },
        amount: Decimal::new(100_000_000, 8), // 1.00 USDT
    };

    // 4. Handle request
    let response = handler.handle_transfer(request).await?;

    // 5. Check result
    if response.status == 0 {
        println!("✅ Transfer successful!");
        println!("Request ID: {:?}", response.data);
    } else {
        println!("❌ Transfer failed: {}", response.msg);
    }

    Ok(())
}
```

---

## Step 4: Query Transfer Status

```rust
// Get transfer by ID
let transfer = db.get_transfer_by_id(request_id).await?;

if let Some(t) = transfer {
    println!("Status: {}", t.status);
    println!("Amount: {}", t.amount);
}
```

---

## Step 5: Run Tests

```bash
# Unit tests
cargo test --lib

# Specific module
cargo test models::internal_transfer_types

# API tests
cargo test api::internal_transfer
```

---

## Common Issues

### Issue: "Failed to connect to ScyllaDB"
**Solution:** Ensure ScyllaDB is running on `localhost:9042`

### Issue: "Unknown asset: XYZ"
**Solution:** Check `SymbolManager::load_from_db()` - only BTC, USDT, ETH supported

### Issue: "ASSET_MISMATCH error"
**Solution:** from_account and to_account must use the same asset

---

## API Endpoints (Future)

```http
# Create transfer
POST /api/v1/user/internal_transfer
Content-Type: application/json

{
  "from_account": { "account_type": "funding", "asset": "USDT" },
  "to_account": { "account_type":" spot", "user_id": 3001, "asset": "USDT" },
  "amount": "1.00"
}

# Query transfer
GET /api/v1/user/internal_transfer/{request_id}

# Query history
GET /api/v1/user/internal_transfer/history?limit=20
```

---

## Next Steps

1. **Integrate TigerBeetle** for balance management
2. **Add Aeron** for UBSCore communication
3. **Implement Settlement** scanning and recovery
4. **Add monitoring** and logging

---

**Status:** MVP Complete ✅
**Production Ready:** 🟡 Partial (missing TB/Aeron integration)
