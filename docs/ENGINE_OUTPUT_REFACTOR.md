# Engine Output Refactor: Atomic Chain-Hashed Output Bundle

## Overview

Refactor the Matching Engine output format to provide atomic, verifiable, chain-linked output bundles. Each output contains the original input plus all effects, enabling:
- Atomic processing (all or nothing)
- Idempotency (input_seq as key)
- Integrity verification (CRC, hash)
- Chain verification (prev_hash linkage)
- Complete audit trail

## Current Architecture (Problems)

```
Order Input
    ↓
Matching Engine
    ↓ (Multiple separate messages)
    ├── LedgerCommand::OrderUpdate
    ├── LedgerCommand::Lock
    ├── LedgerCommand::MatchExecBatch
    └── LedgerCommand::Deposit/Withdraw/etc.
    ↓
Settlement Service (processes each separately)
    → Risk: partial processing, duplicates, ordering issues
```

## New Architecture

```
Order Input
    ↓
Matching Engine
    ↓ (Single atomic bundle)
    EngineOutput {
        output_seq, prev_hash, hash,
        input: { bytes, crc, seq },
        outputs: { order_updates, trades, balance_events }
    }
    ↓
Settlement Service
    1. Verify chain (prev_hash == last_stored_hash)
    2. Verify integrity (hash, crc)
    3. Process atomically (all outputs in one transaction)
    4. Store last_hash for next verification
```

## Data Structures

### EngineOutput (Main Bundle)

```rust
use sha2::{Sha256, Digest};

/// Complete output bundle from Matching Engine
/// One input → One EngineOutput (with all effects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineOutput {
    // === CHAIN HEADER ===
    /// Monotonically increasing output sequence
    pub output_seq: u64,

    /// Hash of previous EngineOutput (chain linkage)
    /// Genesis output has prev_hash = [0u8; 32]
    pub prev_hash: [u8; 32],

    // === INPUT (Self-contained) ===
    /// Original input data (serialized)
    pub input: InputBundle,

    // === OUTPUTS ===
    /// Order status update
    pub order_update: Option<OrderUpdate>,

    /// Trades generated (0, 1, or many)
    pub trades: Vec<TradeOutput>,

    /// Balance changes (all in one place)
    pub balance_events: Vec<BalanceEvent>,

    // === SELF HASH ===
    /// SHA256 of (output_seq || prev_hash || input || outputs)
    pub hash: [u8; 32],
}

/// Original input with integrity check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputBundle {
    /// Input sequence (idempotency key)
    pub input_seq: u64,

    /// CRC32 of input_data bytes
    pub crc: u32,

    /// Original input data
    pub data: InputData,
}

/// All possible input types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputData {
    PlaceOrder(PlaceOrderInput),
    CancelOrder(CancelOrderInput),
    Deposit(DepositInput),
    Withdraw(WithdrawInput),
}

/// Place order input (full original data)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceOrderInput {
    pub order_id: u64,
    pub user_id: u64,
    pub symbol_id: u32,
    pub side: u8,           // 1=Buy, 2=Sell
    pub order_type: u8,     // 1=Limit, 2=Market
    pub price: u64,
    pub quantity: u64,
    pub cid: String,        // Client order ID
    pub created_at: u64,
}

/// Trade output (derived from MatchExecData)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeOutput {
    pub trade_id: u64,
    pub match_seq: u64,
    pub buy_order_id: u64,
    pub sell_order_id: u64,
    pub buyer_user_id: u64,
    pub seller_user_id: u64,
    pub price: u64,
    pub quantity: u64,
    pub base_asset_id: u32,
    pub quote_asset_id: u32,
    pub buyer_refund: u64,
    pub seller_refund: u64,
    pub settled_at: u64,
}

/// Balance event (unified format for all balance changes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEvent {
    pub user_id: u64,
    pub asset_id: u32,
    pub seq: u64,           // Balance sequence (version)
    pub delta_avail: i64,   // Change to available
    pub delta_frozen: i64,  // Change to frozen
    pub avail: i64,         // Balance after
    pub frozen: i64,        // Frozen after
    pub event_type: String, // "deposit", "lock", "trade_credit", etc.
    pub ref_id: u64,        // Order ID or Trade ID
}

/// Order status update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub order_id: u64,
    pub user_id: u64,
    pub status: u8,         // 1=Accepted, 2=PartialFill, 3=Filled, 4=Cancelled, 5=Rejected
    pub filled_qty: u64,
    pub remaining_qty: u64,
    pub updated_at: u64,
}
```

### Hash Computation

```rust
impl EngineOutput {
    /// Compute hash for this output
    pub fn compute_hash(&self) -> [u8; 32] {
        // TODO: not use Sha256, but use the hash as prev arch design doc
        let mut hasher = Sha256::new();

        // Chain: output_seq + prev_hash
        hasher.update(self.output_seq.to_le_bytes());
        hasher.update(&self.prev_hash);

        // Input
        hasher.update(self.input.input_seq.to_le_bytes());
        hasher.update(self.input.crc.to_le_bytes());
        // ... serialize input.data

        // Outputs
        if let Some(ref ou) = self.order_update {
            hasher.update(ou.order_id.to_le_bytes());
            // ... all fields
        }
        for trade in &self.trades {
            hasher.update(trade.trade_id.to_le_bytes());
            // ... all fields
        }
        for event in &self.balance_events {
            hasher.update(event.user_id.to_le_bytes());
            // ... all fields
        }

        hasher.finalize().into()
    }

    /// Verify this output's integrity
    pub fn verify(&self) -> bool {
        self.hash == self.compute_hash()
    }

    /// Verify chain linkage with previous output
    pub fn verify_chain(&self, prev: &EngineOutput) -> bool {
        self.output_seq == prev.output_seq + 1 &&
        self.prev_hash == prev.hash
    }
}
```

## Matching Engine Changes

### Current Flow (matching_engine_base.rs)

```rust
// Current: Sends multiple LedgerCommands
fn process_order(&mut self, order: &InternalOrder) {
    // Send Lock
    self.send_ledger_command(LedgerCommand::Lock { ... });

    // Process matches
    for trade in trades {
        // Send trade
        batch.push(trade);
    }
    self.send_ledger_command(LedgerCommand::MatchExecBatch(batch));

    // Send order update
    self.send_ledger_command(LedgerCommand::OrderUpdate(...));
}
```

### New Flow

```rust
// New: Builds single EngineOutput
fn process_order(&mut self, order: &InternalOrder) -> EngineOutput {
    let mut output = EngineOutput {
        output_seq: self.next_output_seq(),
        prev_hash: self.last_output_hash,
        input: InputBundle {
            input_seq: order.input_seq,
            crc: compute_crc(&order),
            data: InputData::PlaceOrder(order.into()),
        },
        order_update: None,
        trades: vec![],
        balance_events: vec![],
        hash: [0u8; 32], // Computed at end
    };

    // Lock funds
    output.balance_events.push(BalanceEvent {
        user_id: order.user_id,
        asset_id: lock_asset,
        seq: self.ledger.get_next_seq(order.user_id, lock_asset),
        delta_avail: -(lock_amount as i64),
        delta_frozen: lock_amount as i64,
        avail: new_avail,
        frozen: new_frozen,
        event_type: "lock".into(),
        ref_id: order.order_id,
    });

    // Match trades
    for trade in self.match_order(order) {
        output.trades.push(trade.into());

        // Add trade balance events
        output.balance_events.push(/* buyer credit */);
        output.balance_events.push(/* buyer debit */);
        output.balance_events.push(/* seller credit */);
        output.balance_events.push(/* seller debit */);
    }

    // Order status
    output.order_update = Some(OrderUpdate { ... });

    // Compute hash
    output.hash = output.compute_hash();
    self.last_output_hash = output.hash;

    output
}
```

## Settlement Service Changes

### Current Flow

```rust
// Current: Processes each command separately
match cmd {
    LedgerCommand::OrderUpdate(o) => process_order_update(o),
    LedgerCommand::Lock { .. } => settlement_db.lock(...),
    LedgerCommand::MatchExecBatch(trades) => {
        for trade in trades {
            settlement_db.settle_trade_atomically(&trade);
        }
    },
}
```

### New Flow

```rust
// New: Process entire bundle atomically
async fn process_engine_output(
    db: &SettlementDb,
    output: &EngineOutput,
    last_hash: &mut [u8; 32],
) -> Result<()> {
    // 1. Verify chain
    if output.prev_hash != *last_hash {
        // Gap detected! Request missing outputs from ME
        return Err(ChainGap(output.output_seq));
    }

    // 2. Verify integrity
    if !output.verify() {
        return Err(HashMismatch);
    }

    // 3. Idempotency check
    if db.output_exists(output.output_seq).await? {
        log::info!("Output {} already processed, skipping", output.output_seq);
        return Ok(());
    }

    // 4. Process atomically (all or nothing)
    let tx = db.begin_transaction().await?;

    // 4a. Write order to order_history (from input)
    if let InputData::PlaceOrder(ref order) = output.input.data {
        tx.insert_order_history(order).await?;
    }

    // 4b. Write order update
    if let Some(ref update) = output.order_update {
        tx.update_order_status(update).await?;
    }

    // 4c. Write trades
    for trade in &output.trades {
        tx.insert_trade(trade).await?;
    }

    // 4d. Write balance events
    for event in &output.balance_events {
        tx.insert_balance_event(event).await?;
    }

    // 4e. Mark output as processed
    tx.mark_output_processed(output.output_seq, output.hash).await?;

    tx.commit().await?;

    // 5. Update chain state
    *last_hash = output.hash;

    Ok(())
}
```

## Database Schema Changes

### New Table: engine_outputs

```sql
CREATE TABLE IF NOT EXISTS engine_outputs (
    output_seq bigint PRIMARY KEY,
    prev_hash blob,
    hash blob,
    input_seq bigint,
    processed_at bigint,

    -- Index for chain verification
    INDEX idx_hash (hash)
);
```

### Simplified balance_ledger

The `balance_ledger` table stays the same, but now ALL balance events come from `EngineOutput.balance_events`.

## Migration Plan

### Phase 1: Add EngineOutput structure ✅ COMPLETED
- [x] Create new data structures in `src/engine_output.rs`
- [x] Add hash computation functions (xxh3_64)
- [x] Add serialization (serde JSON)
- [x] Add `EngineOutputBuilder` for incremental construction
- [x] Add unit tests for hashing, chain verification, CRC

### Phase 2: Update Matching Engine ✅ COMPLETED
- [x] Add `last_output_hash` field to `MatchingEngine` struct
- [x] Add `last_output_hash` to `EngineSnapshot` for persistence
- [x] Add `get_frozen` method to `Ledger` trait for frozen balance queries
- [x] Implement `process_order_with_output()` - builds `EngineOutput` with all effects
- [x] Add public `add_order_and_build_output()` method for external access
- [x] Chain hash state maintained (`last_output_hash` updated after each output)
- [x] Balance events captured: lock, trade_credit, trade_debit, trade_refund

### Phase 3: Update Settlement Service ✅ COMPLETED
- [x] Add chain verification logic (`verify_prev_hash`, `verify` hash)
- [x] Add sequence continuity verification (with warning on gaps)
- [x] Process `EngineOutput` atomically in `process_engine_output()`
- [x] Track `last_processed_hash` and `last_output_seq` for chain state
- [x] Process balance events (deposit, unlock, withdraw from EngineOutput)
- [x] Process trades via `settle_trade_atomically`
- [x] Process order history updates
- [x] Fallback to LedgerCommand for backward compatibility

### Phase 4: Update ZMQ Protocol (PARTIAL)
- [x] Add `publish_engine_output()` method to `ZmqPublisher`
- [x] JSON serialization for `EngineOutput` (via serde_json)
- [x] Add `EngineOutput` import to `matching_engine_server.rs`
- [x] Add `engine_outputs` field to `OrderEvent` struct (prepared for migration)
- [x] Add migration notes/comments in server code
- [ ] *(Future)* Enable single-order `EngineOutput` processing in Disruptor consumer
- Note: Current architecture uses batched processing with `add_order_batch`. Full migration requires
  switching to `add_order_and_build_output()` per order.

### Phase 5: Cleanup (PARTIAL)
- [x] Add deprecation markers to `LedgerCommand::TradeSettle` (never used)
- [x] Add documentation comments to `LedgerCommand` variants
- [x] Add integration tests for EngineOutput flow (`tests/tests_engine_output_flow.rs`)
- [ ] Enable EngineOutput flow in server (when ready)
- [ ] Remove old `LedgerCommand` variants (requires full migration)
- [ ] Remove duplicate balance event logic from `settle_trade_atomically`
- [ ] Simplify settlement code

## Integration Tests

Three integration tests verify the full EngineOutput flow:

1. **`test_matching_engine_produces_valid_engine_output`**
   - Verifies `add_order_and_build_output` produces valid `EngineOutput`
   - Confirms hash verification, input CRC, order update, balance events

2. **`test_engine_output_chain_verification`**
   - Verifies chain linkage between consecutive outputs
   - Confirms trade matching produces correct `TradeOutput`
   - Verifies `prev_hash` chains correctly

3. **`test_settlement_chain_verification_logic`**
   - Simulates settlement service verification
   - Tests `verify_prev_hash`, `verify`, sequence checks
   - Verifies broken chain detection

## Benefits Summary

| Feature | Before | After |
|---------|--------|-------|
| Atomicity | Partial (multiple messages) | Full (single bundle) |
| Idempotency | Per-command | Per-output (chain-linked) |
| Verification | None | CRC + Hash + Chain |
| Order History | Separate flow | From input bundle |
| Balance Events | Computed twice | Single source |
| Gap Detection | Manual | Automatic (prev_hash) |

## Timeline Estimate

- Phase 1: 2 hours (data structures) ✅
- Phase 2: 4 hours (ME changes) ✅
- Phase 3: 3 hours (Settlement changes) ✅
- Phase 4: 1 hour (ZMQ protocol) ✅ (partial)
- Phase 5: 2 hours (cleanup) 🟡 (partial)
- Testing: 4 hours ✅

**Total: ~16 hours - Core refactor complete, full migration pending**
