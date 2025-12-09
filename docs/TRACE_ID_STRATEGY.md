# Trace ID Strategy - Quick Reference

## Decision Matrix: When to Use trace_id

### ✅ Use Natural ID (NO trace_id needed)

| Event Type | Natural ID | Example |
|-----------|------------|---------|
| Order operations | `order_id` | `order_12345_1733856600000` |
| Trade operations | `trade_id` | `trade_98765_1733856600000` |
| Deposit/Withdrawal | `event_id` | `deposit_1001_1_1733856600000` |
| User operations | `user_id` | `user_1001` |

**Rule**: If operation has a unique business ID, use it directly. No need for trace_id.

```rust
// ✅ Good - use order_id
info!("event=ORDER_MATCHED order_id={}", order_id);

// ❌ Bad - redundant trace_id
info!("trace_id={} order_id={}", trace_id, order_id);
```

---

### ⚠️ Generate trace_id When:

| Scenario | Format | Example |
|----------|--------|---------|
| Batch operations | `batch_{seq}_{timestamp}` | `batch_12345_1733856600000` |
| Multi-step workflows | `flow_{type}_{hash}_{timestamp}` | `flow_reg_a1b2c3_1733856600000` |
| Distributed requests | `req_{seq}_{timestamp}` | `req_98765_1733856600000` |
| User sessions | `session_{sid}_{timestamp}` | `session_xyz_1733856600000` |

**Rule**: Generate trace_id when multiple events form a logical transaction but have no single natural ID.

---

## Examples

### ✅ Example 1: Order Flow (Use order_id)

```rust
// Gateway
info!("event=ORDER_RECEIVED order_id={}", order_id);

// UBSCore
info!("event=ORDER_VALIDATED order_id={} balance_ok=true", order_id);

// ME
info!("event=ORDER_MATCHED order_id={} trade_id={}", order_id, trade_id);

// Settlement
info!("event=TRADE_PERSISTED trade_id={} order_id={}", trade_id, order_id);
```

**Tracking**: `grep "order_12345" logs/*.log` shows complete lifecycle

---

### ⚠️ Example 2: Batch Settlement (Generate trace_id)

```rust
// Settlement processes batch of 100 trades
let batch_trace_id = format!("batch_{}_{}", batch_seq, now_ms());

info!("event=BATCH_START trace_id={} size=100", batch_trace_id);

for trade in trades {
    // Each trade ALSO has trade_id
    info!("event=TRADE_SETTLED trace_id={} trade_id={}",
          batch_trace_id, trade.trade_id);
}

info!("event=BATCH_COMPLETE trace_id={} success=95 failed=5", batch_trace_id);
```

**Tracking**:
- `grep "batch_12345" logs/*.log` → Shows batch-level events
- `grep "trade_98765" logs/*.log` → Shows individual trade

---

### ⚠️ Example 3: User Registration Workflow (Generate trace_id)

```rust
// Multi-step process, user_id doesn't exist yet
let flow_trace_id = format!("reg_flow_{}_{}", email_hash, now_ms());

info!("event=REG_START trace_id={} email={}", flow_trace_id, email_hash);
info!("event=EMAIL_SENT trace_id={}", flow_trace_id);
info!("event=EMAIL_VERIFIED trace_id={}", flow_trace_id);
info!("event=KYC_SUBMITTED trace_id={}", flow_trace_id);
info!("event=ACCOUNT_CREATED trace_id={} user_id={}", flow_trace_id, user_id);

// After user_id exists, switch to using it
info!("event=WELCOME_EMAIL_SENT user_id={}", user_id);
```

---

## Format Standards

### trace_id Format
```
{prefix}_{unique_component}_{timestamp_ms}
```

**Rules**:
1. Prefix indicates what it traces (batch, flow, req, session)
2. Unique component (sequence, hash, session_id)
3. Always include timestamp for uniqueness
4. Max 64 characters total
5. Use underscore separators

### Examples
```rust
batch_12345_1733856600000       // ✅ Good
flow_reg_a1b2c3_1733856600000   // ✅ Good
req_98765_1733856600000         // ✅ Good

batch-12345                     // ❌ Bad: no timestamp, wrong separator
trace_123                       // ❌ Bad: no prefix, no timestamp
very_long_trace_id_that_exceeds_64_chars... // ❌ Bad: too long
```

---

## When to Transition from trace_id to Natural ID

### Pattern: Start with trace_id, end with natural ID

```rust
// 1. Start: No ID yet, generate trace_id
let flow_trace_id = gen_trace_id("order_flow");
info!("event=ORDER_VALIDATION_START trace_id={}", flow_trace_id);

// 2. Middle: Still using trace_id
info!("event=BALANCE_CHECK trace_id={} result=ok", flow_trace_id);

// 3. Natural ID created: Include both
let order_id = create_order();
info!("event=ORDER_CREATED trace_id={} order_id={}", flow_trace_id, order_id);

// 4. End: Switch to order_id only
info!("event=ORDER_TO_ME order_id={}", order_id);
info!("event=ORDER_MATCHED order_id={}", order_id);
```

**Why**: Allows tracking entire flow even before natural ID exists, then seamless transition.

---

## Code Helpers

### trace_id Generator
```rust
pub fn gen_trace_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}",  prefix, seq, now_ms())
}

// Usage:
let batch_id = gen_trace_id("batch");      // batch_12345_1733856600000
let flow_id = gen_trace_id("reg_flow");    // reg_flow_12346_1733856600000
```

### event_id vs trace_id Helper
```rust
pub enum EventIdentifier {
    // Use when operation has natural unique ID
    Natural {
        id_type: &'static str,  // "order", "trade", "deposit"
        id: String
    },

    // Use when multiple events form transaction
    Traced {
        trace_id: String
    },
}

impl EventIdentifier {
    pub fn order(order_id: u64) -> Self {
        Self::Natural {
            id_type: "order",
            id: order_id.to_string()
        }
    }

    pub fn batch() -> Self {
        Self::Traced {
            trace_id: gen_trace_id("batch")
        }
    }
}
```

---

## Summary

### Simple Rules:

1. **Has unique business ID?** → Use it (order_id, trade_id, deposit event_id)
2. **Batch/workflow/session?** → Generate trace_id
3. **trace_id format**: `{prefix}_{seq}_{timestamp}`
4. **Include both**: During transition from trace_id → natural ID
5. **Max 64 chars**: Keep IDs concise

### When in doubt:
- Single operation → Natural ID
- Multiple operations → trace_id
- Unclear → Use trace_id (can always add natural ID later)

---

**Remember**: The goal is **traceability**. Use the simplest ID that achieves this!
