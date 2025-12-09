# Logging Phase 3 - Production Hardening Recommendations

## Overview

This document outlines the next phase of logging improvements to make the system production-ready. Based on professional best practices and performance requirements.

---

## 1. Structured JSON Logging ⭐ **HIGHEST PRIORITY**

### Why
- Machine-parseable logs for ELK/CloudWatch/Splunk
- Type-safe queries on log data
- Better integration with monitoring tools

### Current State
```rust
info!("[DEPOSIT_CONSUMED] event_id={} user={} asset={} amount={}",
      event_id, user_id, asset_id, amount);
```

### Target State
```rust
use serde_json::json;

info!(
    target: "deposit_flow",
    "{}",
    json!({
        "event": "DEPOSIT_CONSUMED",
        "event_id": event_id,
        "user_id": user_id,
        "asset_id": asset_id,
        "amount": amount,
        "timestamp_ms": now_ms(),
        "service": "ubscore",
        "host": hostname()
    })
);
```

### Implementation
Create `src/logging.rs`:

```rust
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub fn hostname() -> String {
    hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Structured log event builder
pub struct LogEvent {
    event: String,
    fields: serde_json::Map<String, Value>,
}

impl LogEvent {
    pub fn new(event: &str) -> Self {
        let mut fields = serde_json::Map::new();
        fields.insert("event".to_string(), json!(event));
        fields.insert("timestamp_ms".to_string(), json!(now_ms()));
        fields.insert("host".to_string(), json!(hostname()));

        Self {
            event: event.to_string(),
            fields,
        }
    }

    pub fn field(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.fields.insert(key.to_string(), value.into());
        self
    }

    pub fn build(self) -> Value {
        Value::Object(self.fields)
    }
}

// Usage:
LogEvent::new("DEPOSIT_CONSUMED")
    .field("event_id", event_id)
    .field("user_id", user_id)
    .field("amount", amount)
    .build()
```

### Benefits
- ✅ ElasticSearch ready (JSON ingest)
- ✅ SQL queries: `SELECT * FROM logs WHERE user_id = 1001`
- ✅ Type validation at parse time
- ✅ Automatic schema discovery

---

## 2. Async Logging ⭐ **PERFORMANCE CRITICAL**

### Why
Disk I/O is slow (~1-10ms). Don't block your hot path.

### Current Impact
```rust
info!(...);  // Blocks thread for 1-10ms while writing to disk
```

At 10K ops/sec with 1ms logging = **10 seconds of blocked time per second!**

### Solution: Tokio Tracing + Non-Blocking Appender

Add to `Cargo.toml`:
```toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
tracing-appender = "0.2"
```

In `main.rs`:
```rust
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter};

fn setup_logging() -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily("logs", "ubscore.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer().with_writer(non_blocking).json());

    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set subscriber");

    guard  // IMPORTANT: Keep guard alive for entire program
}

fn main() {
    let _guard = setup_logging();
    // ... rest of program
}
```

### Performance Gain
- Before: 1-10ms blocking per log
- After: **< 1µs** (just writes to channel)
- 1000x - 10,000x faster logging!

---

## 3. Log Rotation ⭐ **OPERATIONAL REQUIREMENT**

### Why
Without rotation, logs will fill disk and crash the system.

### Implementation
Already included in async logging setup above:

```rust
use tracing_appender::rolling::{RollingFileAppender, Rotation};

let file_appender = RollingFileAppender::new(
    Rotation::DAILY,    // New file each day
    "logs",             // Directory
    "ubscore.log"       // Prefix
);
// Creates: logs/ubscore.log.2025-12-10
```

### Cleanup Script
Create `scripts/cleanup_old_logs.sh`:

```bash
#!/bin/bash
# Keep last 7 days of logs, delete older

LOGS_DIR="./logs"
RETENTION_DAYS=7

find "$LOGS_DIR" -name "*.log.*" -type f -mtime +$RETENTION_DAYS -delete

echo "✅ Cleaned logs older than $RETENTION_DAYS days"
```

Add to crontab:
```bash
# Run daily at 2 AM
0 2 * * * /path/to/cleanup_old_logs.sh
```

### Disk Space Estimation
```
10K events/sec × 200 bytes/event × 86400 sec/day = 17 GB/day
With 7-day retention = ~120 GB
```

---

## 4. Trace ID Strategy

### When to Use trace_id vs Natural IDs

#### ✅ Use Natural ID (No trace_id needed):
```rust
// Orders: order_id is already unique
info!("event_id=order_{}_123456", order_id);

// Trades: trade_id is unique
info!("event_id=trade_{}_123456", trade_id);

// Deposits/Withdrawals: event_id we generate IS the trace
info!("event_id=deposit_{user}_{asset}_{timestamp}");
```

#### ⚠️ Generate trace_id When:
1. **Multiple events form a transaction** (no single ID)
2. **Batch operations** (many items processed together)
3. **Internal workflows** (multi-step processes)

### Trace ID Generation Strategy

#### For Batch Operations:
```rust
// Settlement processing a batch of trades
let batch_trace_id = format!("batch_{}_{}", batch_seq, now_ms());

for trade in trades {
    info!(
        "event=TRADE_SETTLED batch_trace_id={} trade_id={} ...",
        batch_trace_id, trade.trade_id
    );
}
```

#### For Multi-Step Workflows:
```rust
// User registration flow (no single ID until complete)
let flow_trace_id = format!("reg_flow_{}_{}", email_hash, now_ms());

info!("event=REG_START trace_id={} email={}", flow_trace_id, email);
info!("event=EMAIL_VERIFIED trace_id={} ...", flow_trace_id);
info!("event=KYC_CHECK trace_id={} ...", flow_trace_id);
info!("event=ACCOUNT_CREATED trace_id={} user_id={}", flow_trace_id, user_id);
// After this point, use user_id instead of trace_id
```

#### For Distributed Request Chains:
```rust
// API Gateway → Auth → UBSCore → Settlement
// Generate trace_id at entry point (Gateway)

// Gateway:
let trace_id = format!("req_{}_{}", request_seq, now_ms());
headers.insert("X-Trace-ID", trace_id.clone());

// All downstream services use same trace_id
info!("event=AUTH_CHECK trace_id={} ...", trace_id);
info!("event=ORDER_VALIDATED trace_id={} order_id={} ...", trace_id, order_id);
```

### Trace ID Format Recommendations

```rust
// Format: {prefix}_{unique_component}_{timestamp_ms}

// Examples:
batch_12345_1733856600000     // Batch processing
flow_a1b2c3_1733856600000     // Multi-step workflow
req_98765_1733856600000       // Distributed request
session_xyz_1733856600000     // User session

// Rules:
// 1. Prefix indicates what it traces
// 2. Unique component (seq, hash, session_id)
// 3. Always include timestamp for uniqueness
// 4. Max 64 chars total
```

### When NOT to Use trace_id

```rust
// ❌ DON'T: Redundant with order_id
info!("trace_id={} order_id={}", trace_id, order_id);

// ✅ DO: Just use order_id
info!("order_id={} ...", order_id);

// ❌ DON'T: Generate trace_id for single atomic events
let trace_id = gen_trace_id();
info!("trace_id={} event=SINGLE_EVENT", trace_id);

// ✅ DO: Event itself is the trace
info!("event=DEPOSIT_PROCESSED event_id={}", event_id);
```

---

## 5. Event Sampling for High-Volume Operations

### Why
At 100K orders/sec, logging every validation = 100K log lines/sec = TB/day

### Strategy: Sample by ID or Probability

```rust
// Sample 1% of order validations
if order_id % 100 == 0 {
    info!("event=ORDER_VALIDATED order_id={} ...", order_id);
}

// OR: Probabilistic sampling
use rand::Rng;
if rand::thread_rng().gen_bool(0.01) {  // 1% chance
    info!("event=ORDER_VALIDATED ...");
}
```

### What to ALWAYS Log (No Sampling)

```rust
// ✅ ALWAYS log these (no sampling):
// 1. Entry/Exit of major operations
info!("event=DEPOSIT_ENTRY ...");
info!("event=DEPOSIT_EXIT ...");

// 2. State changes
info!("event=BALANCE_UPDATED before={} after={}", ...);

// 3. Errors
error!("event=DEPOSIT_FAILED ...");

// 4. Database writes
info!("event=TRADE_PERSISTED ...");
```

### What to Sample

```rust
// ⚠️ Sample these (high volume, low info):
// 1. Intermediate validations
if sample() { info!("event=CHECKSUM_VALIDATED ..."); }

// 2. Cache hits
if sample() { info!("event=CACHE_HIT ..."); }

// 3. Heartbeats
if seq % 100 == 0 { info!("event=HEARTBEAT ..."); }
```

---

## 6. Metrics Integration

### Why
- Logs = WHAT happened (events)
- Metrics = HOW MUCH (numbers)

Use both together!

### Add to `Cargo.toml`:
```toml
[dependencies]
metrics = "0.21"
metrics-exporter-prometheus = "0.12"
```

### Implementation

```rust
use metrics::{counter, gauge, histogram};
use std::sync::atomic::{AtomicU64, Ordering};
use once_cell::sync::Lazy;

// Counters
static DEPOSITS_TOTAL: Lazy<AtomicU64> = Lazy::new(|| AtomicU64::new(0));
static DEPOSITS_FAILED: Lazy<AtomicU64> = Lazy::new(|| AtomicU64::new(0));

fn process_deposit(...) {
    let start = Instant::now();

    match do_deposit() {
        Ok(_) => {
            DEPOSITS_TOTAL.fetch_add(1, Ordering::Relaxed);
            counter!("deposits_processed_total").increment(1);
        }
        Err(e) => {
            DEPOSITS_FAILED.fetch_add(1, Ordering::Relaxed);
            counter!("deposits_failed_total").increment(1);
            error!("event=DEPOSIT_FAILED ...");
        }
    }

    histogram!("deposit_latency_ms").record(start.elapsed().as_millis() as f64);
}

// Periodic metrics log (every 10 seconds)
tokio::spawn(async {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;

        let total = DEPOSITS_TOTAL.load(Ordering::Relaxed);
        let failed = DEPOSITS_FAILED.load(Ordering::Relaxed);

        info!(
            "{}",
            json!({
                "event": "METRICS_REPORT",
                "deposits_total": total,
                "deposits_failed": failed,
                "success_rate": (total - failed) as f64 / total as f64
            })
        );
    }
});
```

### Prometheus Endpoint
```rust
use metrics_exporter_prometheus::PrometheusBuilder;

// In main():
let builder = PrometheusBuilder::new();
builder
    .with_http_listener(([0, 0, 0, 0], 9090))
    .install()
    .expect("Failed to install Prometheus exporter");

// Metrics available at: http://localhost:9090/metrics
```

---

## 7. PII Redaction & Security

### Critical Rules

```rust
// ❌ NEVER log these:
// - Passwords
// - API keys
// - Session tokens
// - Private keys
// - Credit card numbers
// - Social security numbers

// ⚠️ REDACT in production:
// - Email addresses
// - IP addresses
// - Phone numbers
// - Full names

// ✅ SAFE to log:
// - user_id (numeric)
// - order_id
// - asset_id
// - amounts (for financial auditing)
// - timestamps
```

### Implementation

```rust
#[cfg(debug_assertions)]
fn maybe_redact(value: &str) -> String {
    value.to_string()
}

#[cfg(not(debug_assertions))]
fn maybe_redact(_value: &str) -> String {
    "***REDACTED***".to_string()
}

// Usage:
info!(
    "event=USER_LOGIN email={} ip={}",
    maybe_redact(&email),
    maybe_redact(&ip)
);
```

---

## 8. Log Level Guidelines

### Use Appropriate Levels

```rust
// ERROR: System failures requiring IMMEDIATE action
error!("event=DATABASE_DOWN ...");
error!("event=KAFKA_DISCONNECTED ...");
error!("event=DEPOSIT_FAILED reason=insufficient_balance ...");

// WARN: Unexpected but handled (investigate soon)
warn!("event=SLOW_QUERY duration_ms=500 threshold=100 ...");
warn!("event=RETRY_ATTEMPT attempt=2 max=3 ...");
warn!("event=CACHE_MISS ...");

// INFO: Normal business events (current usage ✅)
info!("event=DEPOSIT_CONSUMED ...");
info!("event=ORDER_MATCHED ...");
info!("event=TRADE_SETTLED ...");

// DEBUG: Development/troubleshooting (disabled in prod)
debug!("event=ENTERING_FUNCTION fn=process_deposit ...");
debug!("event=VARIABLE_VALUE var=total value={}", total);

// TRACE: Very verbose (almost never used)
trace!("event=LOOP_ITERATION i={}", i);
```

### Production Environment Variables

```bash
# Production
RUST_LOG=info,ubscore=info,settlement=info

# Debugging specific issue
RUST_LOG=info,ubscore::deposit=debug

# Performance testing
RUST_LOG=warn
```

---

## 9. Error Context with anyhow

### Current Pattern
```rust
let event_json = serde_json::to_string(&event).unwrap();  // Panics!
```

### Better Pattern
```rust
use anyhow::{Context, Result};

let event_json = serde_json::to_string(&event)
    .context(format!(
        "Failed to serialize balance event: event_id={} user={}",
        event_id, user_id
    ))?;

// If error occurs, you get full context:
// Error: Failed to serialize balance event: event_id=deposit_1001_1_123 user=1001
// Caused by:
//     invalid type: map, expected a string
```

### In Logging
```rust
match process_deposit().context("Deposit processing failed") {
    Ok(_) => info!("event=DEPOSIT_SUCCESS ..."),
    Err(e) => {
        error!(
            "{}",
            json!({
                "event": "DEPOSIT_ERROR",
                "event_id": event_id,
                "error": format!("{:?}", e),  // Full chain
                "error_msg": e.to_string()    // Top-level only
            })
        );
    }
}
```

---

## 10. Log Testing

### Why Test Logs?
- Ensure critical events are logged
- Verify event_id consistency
- Catch logging regressions

### Implementation

```rust
#[cfg(test)]
mod tests {
    use tracing_test::traced_test;

    #[traced_test]
    #[test]
    fn test_deposit_lifecycle_logged() {
        process_deposit(1001, 1, 1000000);

        // Verify all lifecycle events logged
        assert!(logs_contain("DEPOSIT_CONSUMED"));
        assert!(logs_contain("DEPOSIT_VALIDATED"));
        assert!(logs_contain("DEPOSIT_TO_SETTLEMENT"));
        assert!(logs_contain("DEPOSIT_EXIT"));
    }

    #[traced_test]
    #[test]
    fn test_event_id_consistency() {
        process_deposit(1001, 1, 1000000);

        let event_ids = extract_all_event_ids(captured_logs());

        // All logs for this operation should have SAME event_id
        assert!(!event_ids.is_empty());
        assert!(event_ids.windows(2).all(|w| w[0] == w[1]));
    }
}
```

---

## Implementation Priority

### Phase 3 (This Sprint) - CRITICAL
1. ✅ **Structured JSON logging** (2 days)
2. ✅ **Async logging** (1 day)
3. ✅ **Log rotation** (included in #2)
4. ✅ **Metrics integration** (2 days)

**Total: ~5 days, 80% production ready**

### Phase 4 (Next Sprint) - IMPORTANT
5. ✅ **Event sampling** (1 day)
6. ✅ **PII redaction** (1 day)
7. ✅ **Error context** (0.5 days - refactoring)

**Total: ~2.5 days, 95% production ready**

### Phase 5 (Future) - NICE TO HAVE
8. ✅ **Log testing** (ongoing)
9. ✅ **ELK integration** (1 week)
10. ✅ **Alerting** (1 week)

---

## Quick Start: Add This First

Create `src/logging.rs` with the helper functions:

```rust
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub struct StructuredLog;

impl StructuredLog {
    pub fn deposit_consumed(event_id: &str, user_id: u64, asset_id: u32, amount: u64) -> Value {
        json!({
            "event": "DEPOSIT_CONSUMED",
            "event_id": event_id,
            "user_id": user_id,
            "asset_id": asset_id,
            "amount": amount,
            "timestamp_ms": now_ms(),
            "service": "ubscore"
        })
    }

    // Add more as needed...
}

// Usage:
info!("{}", StructuredLog::deposit_consumed(&event_id, user_id, asset_id, amount));
```

---

## Summary

Your current logging (Phase 2) is **solid** for debugging.
These Phase 3 improvements make it **production-grade** for:
- ✅ Scale (async logging)
- ✅ Operations (rotation, metrics)
- ✅ Analysis (JSON structure)
- ✅ Security (PII redaction)
- ✅ Tracing (smart use of IDs)

**Recommendation**: Start with JSON + async logging (biggest wins for minimal effort).
