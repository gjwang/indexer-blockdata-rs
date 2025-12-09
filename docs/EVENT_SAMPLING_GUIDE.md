# Event Sampling - Future Optimization Guide

## Overview

**Event sampling** is a performance optimization for high-volume logging where you log only a percentage of events instead of every single occurrence. This is a **future optional enhancement** to reduce log volume while maintaining observability.

---

## When to Implement

Implement event sampling when:
- ✅ Your system processes **>10K events/second** of the same type
- ✅ Logs are growing **>50GB/day**
- ✅ You're spending significant money on log storage
- ✅ It's hard to find important events in the noise

**Current Status**: Not needed yet - defer until you have volume issues

---

## The Problem

### Example: High-Volume Order Validation

```rust
// Current implementation - logs EVERY order validation
for order in orders {
    validate_order(&order);
    tracing::info!("ORDER_VALIDATED order_id={}", order.id);
}

// At 100K orders/second:
// - 100,000 log lines/second
// - 17 GB of logs per day
// - Expensive to store and query
// - Hard to find actual errors
```

---

## The Solution

### Sample Only 1% of Validations

```rust
// Log only every 100th validation (1%)
for order in orders {
    validate_order(&order);

    if order.id % 100 == 0 {
        tracing::info!("ORDER_VALIDATED order_id={}", order.id);
    }
}

// Result:
// - 1,000 log lines/second (instead of 100K)
// - 170 MB/day (instead of 17 GB)
// - 99% cost reduction
// - Still enough samples to monitor health
```

---

## Implementation Strategies

### 1. Modulo Sampling (Recommended)

**Deterministic** - Same IDs always logged

```rust
// Helper function in src/logging.rs
pub fn should_sample(id: u64, rate: u64) -> bool {
    id % rate == 0
}

// Sample 1% (every 100th)
if should_sample(order_id, 100) {
    tracing::debug!("ORDER_VALIDATED order_id={}", order_id);
}

// Sample 0.1% (every 1000th)
if should_sample(order_id, 1000) {
    tracing::debug!("CHECKSUM_VERIFIED order_id={}", order_id);
}
```

**Pros**:
- Fast (just modulo operation)
- Deterministic (same IDs always logged)
- No dependencies

**Cons**:
- Not truly random (could miss patterns)

### 2. Probabilistic Sampling

**Random** - Each event has X% chance of being logged

```rust
use rand::Rng;

// Sample 1% randomly
if rand::thread_rng().gen_bool(0.01) {
    tracing::debug!("ORDER_VALIDATED order_id={}", order_id);
}

// Sample 0.1% randomly
if rand::thread_rng().gen_bool(0.001) {
    tracing::debug!("CACHE_HIT key={}", key);
}
```

**Pros**:
- Truly random distribution
- Better statistical properties

**Cons**:
- Requires RNG (slight overhead)
- Non-deterministic (different IDs each time)

### 3. Rate Limiting (Time-Based)

**Bounded** - Maximum N logs per second

```rust
use std::sync::atomic::{AtomicU64, Ordering};

static LAST_VALIDATION_LOG: AtomicU64 = AtomicU64::new(0);

fn log_validation_sampled(order_id: u64) {
    let now = now_ms();
    let last = LAST_VALIDATION_LOG.load(Ordering::Relaxed);

    // Log at most 10 per second
    if now - last >= 100 {  // 100ms interval
        LAST_VALIDATION_LOG.store(now, Ordering::Relaxed);
        tracing::debug!("ORDER_VALIDATED order_id={}", order_id);
    }
}
```

**Pros**:
- Bounded rate regardless of volume
- Predictable log output

**Cons**:
- More complex
- Requires atomic operations

---

## Decision Matrix: What to Sample

### ❌ **NEVER Sample These** (Always Log 100%)

| Event Type | Why |
|------------|-----|
| **Errors** | Every error needs investigation |
| **State Changes** | Balance updates, order fills, trades |
| **Entry/Exit** | Service start/stop, request entry/exit |
| **Database Writes** | Critical persistence operations |
| **Security** | Auth failures, access denied |
| **Money Movement** | Deposits, withdrawals, settlements |

```rust
// ALWAYS log these - no sampling
tracing::error!("[ORDER_REJECTED] order_id={} reason={}", id, reason);
tracing::info!("[TRADE_MATCHED] trade_id={} qty={}", trade_id, qty);
tracing::info!("[DEPOSIT_PERSISTED] event_id={} amount={}", event_id, amount);
```

### ✅ **Sample These** (High Volume, Lower Value)

| Event Type | Sampling Rate | Rationale |
|------------|---------------|-----------|
| **Validations** | 1% (100:1) | High volume, low info value |
| **Cache Hits** | 0.1% (1000:1) | Very high volume |
| **Heartbeats** | 1% or time-based | Periodic, predictable |
| **Intermediate Steps** | 1-10% | Non-critical processing |

```rust
// Sample these - high volume, low value
if should_sample(order_id, 100) {
    tracing::debug!("[ORDER_VALIDATED] order_id={}", order_id);
}

if should_sample(user_id, 1000) {
    tracing::debug!("[CACHE_HIT] user={}", user_id);
}
```

---

## Implementation Plan (Future)

### Phase 1: Add Helper Functions

Add to `src/logging.rs`:

```rust
/// Sample helper - returns true 1/rate times
///
/// Example: should_sample(id, 100) = true 1% of the time
pub fn should_sample(id: u64, rate: u64) -> bool {
    id % rate == 0
}

/// Sample helper with percentage
///
/// Example: sample_percent(id, 1) = true 1% of the time
pub fn sample_percent(id: u64, percent: u64) -> bool {
    should_sample(id, 100 / percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampling_rate() {
        let mut sampled = 0;
        for i in 0..10000 {
            if should_sample(i, 100) {
                sampled += 1;
            }
        }
        assert_eq!(sampled, 100); // Exactly 1%
    }
}
```

### Phase 2: Apply to High-Volume Events

In **Matching Engine** (`matching_engine_server.rs`):

```rust
// Before:
tracing::debug!("Checksum verified: order_id={}", order_id);

// After (1% sample):
if should_sample(order_id, 100) {
    tracing::debug!("Checksum verified: order_id={}", order_id);
}
```

In **UBSCore** (`ubscore_aeron_service.rs`):

```rust
// Before:
tracing::debug!("Balance check passed: user={}", user_id);

// After (1% sample):
if should_sample(user_id, 100) {
    tracing::debug!("Balance check passed: user={}", user_id);
}
```

### Phase 3: Monitor & Tune

1. **Measure current log volume**:
   ```bash
   du -sh logs/
   ```

2. **Apply sampling to high-volume events**

3. **Measure new log volume**:
   ```bash
   du -sh logs/
   ```

4. **Adjust sampling rates** based on volume

---

## Sampling Rates Guide

| Volume | Recommended Rate | Logs/Day (Before) | Logs/Day (After) |
|--------|------------------|-------------------|------------------|
| 10K/sec | 10% (rate=10) | 850 GB | 85 GB |
| 100K/sec | 1% (rate=100) | 8.5 TB | 85 GB |
| 1M/sec | 0.1% (rate=1000) | 85 TB | 85 GB |

**Rule of thumb**: Keep total logs **under 100 GB/day** for cost-effectiveness

---

## Example: Complete Implementation

```rust
// src/logging.rs additions
pub fn should_sample(id: u64, rate: u64) -> bool {
    id % rate == 0
}

pub fn sample_percent(id: u64, percent: u64) -> bool {
    should_sample(id, 100 / percent)
}

// Usage in matching_engine_server.rs
use fetcher::logging::{should_sample, sample_percent};

// Always log critical events
tracing::info!("[ORDER_MATCHED] order_id={} trade_id={}", order_id, trade_id);
tracing::error!("[VALIDATION_FAILED] order_id={} reason={}", order_id, reason);

// Sample high-volume events
if should_sample(order_id, 100) {  // 1%
    tracing::debug!("[CHECKSUM_OK] order_id={}", order_id);
}

if sample_percent(trade_id, 1) {  // 1%
    tracing::debug!("[CACHE_HIT] trade_id={}", trade_id);
}

if should_sample(seq, 1000) {  // 0.1%
    tracing::debug!("[HEARTBEAT] seq={}", seq);
}
```

---

## Monitoring Sampling Effectiveness

### Before Sampling
```bash
# Count logs per second
grep "ORDER_VALIDATED" logs/matching_engine.log | wc -l
# Result: 100,000

# Disk usage
du -sh logs/
# Result: 17 GB/day
```

### After Sampling (1%)
```bash
# Count logs per second
grep "ORDER_VALIDATED" logs/matching_engine.log | wc -l
# Result: 1,000 (99% reduction)

# Disk usage
du -sh logs/
# Result: 170 MB/day (99% reduction)
```

### Verify Samples Are Representative
```bash
# Check distribution of sampled order IDs
grep "ORDER_VALIDATED" logs/matching_engine.log | \
  jq -r '.fields.order_id' | \
  awk '{print $1 % 100}' | \
  sort | uniq -c
# Should all be 0 (every 100th ID)
```

---

## Cost-Benefit Analysis

### Costs
- **Dev time**: 1-2 hours to implement
- **Slight complexity**: More conditional logging
- **Less visibility**: Only see 1% of validations

### Benefits
- **99% log reduction** for high-volume events
- **99% cost savings** on log storage
- **100x faster** log queries
- **Easier to find errors** (less noise)
- **Better performance** (less I/O)

### ROI
If logging 100K events/sec:
- **Before**: $500/month log storage + query costs
- **After**: $5/month log storage + query costs
- **Savings**: $495/month = **$5,940/year**

**ROI**: 2 hours dev time → $6K/year savings = **Excellent ROI**

---

## When NOT to Use Sampling

1. **Low volume systems** (<1K events/sec) - Not worth the complexity
2. **Debug mode** - When actively troubleshooting, log everything
3. **Critical events** - Errors, state changes, money movement
4. **Compliance** - If regulations require full audit trail

---

## Summary

**Event Sampling** = Log only X% of high-volume events

### Quick Reference
- **Purpose**: Reduce log volume by 90-99%
- **When**: At >10K events/sec of same type
- **Method**: `if id % 100 == 0 { log }` for 1% sampling
- **What**: Sample validations, cache hits, heartbeats
- **What NOT**: Never sample errors, trades, deposits
- **Benefit**: 99% cost reduction, easier debugging

### Implementation Checklist
- [ ] Add `should_sample()` helper to `src/logging.rs`
- [ ] Identify high-volume events (>10K/sec)
- [ ] Apply sampling with appropriate rates
- [ ] Measure before/after log volume
- [ ] Tune sampling rates based on results

---

**Status**: 📋 **DOCUMENTED** - Implement when log volume becomes an issue
**Priority**: Low (future optimization)
**Estimated Effort**: 1-2 hours
**Estimated Savings**: $6K/year @ 100K events/sec
**Best Time**: When logs exceed 50 GB/day
