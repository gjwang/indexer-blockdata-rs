# Internal Transfer - Monitoring Guide

## Key Metrics to Track

### 1. Request Metrics
- `internal_transfer_requests_total` - Total number of requests
- `internal_transfer_requests_by_status{status="requesting|pending|success|failed"}` - Requests by status
- `internal_transfer_request_duration_seconds` - Request processing time

### 2. Balance Metrics
- `tigerbeetle_balance_locks_total` - Number of balance locks (PENDING)
- `tigerbeetle_balance_unlocks_total` - Number of unlocks (POST/VOID)
- `tigerbeetle_operations_duration_seconds{op="create_pending|post|void"}` - TB operation latency

### 3. Database Metrics
- `scylla_write_latency_seconds{table="balance_transfer_requests"}` - DB write latency
- `scylla_query_latency_seconds{query="get_transfer_by_id"}` - DB query latency
- `scylla_errors_total` - Database errors

### 4. Error Metrics
- `internal_transfer_validation_errors_total{code="INVALID_AMOUNT|ASSET_MISMATCH|..."}` - Validation errors
- `internal_transfer_tb_errors_total{error="insufficient_balance|timeout"}` - TB errors
- `internal_transfer_settlement_lag_seconds` - Time between PENDING and POST

## Alert Rules

### Critical Alerts

```yaml
# High failure rate
- alert: HighTransferFailureRate
  expr: rate(internal_transfer_requests_by_status{status="failed"}[5m]) > 0.05
  for: 2m
  labels:
    severity: critical
  annotations:
    summary: "High internal transfer failure rate"
```

```yaml
#  Stuck transfers
- alert: StuckPendingTransfers
  expr: count(balance_transfer_requests{status="pending", updated_at < now() - 5m}) > 10
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "Many transfers stuck in PENDING state"
```

### Warning Alerts

```yaml
# Slow processing
- alert: SlowTransferProcessing
  expr: histogram_quantile(0.95, internal_transfer_request_duration_seconds) > 1.0
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "95th percentile transfer processing time > 1s"
```

## Dashboard Panels

### Overview Panel
- Request rate (requests/sec)
- Success rate (%)
- P50/P95/P99 latency
- Active PENDING transfers

### Status Panel
- Pie chart: Requests by status
- Time series: Status transitions over time

### Performance Panel
- Latency histogram
- DB latency
- TB operation latency

### Errors Panel
- Error rate by type
- Recent errors (table)
- Error codes distribution

## Logging Best Practices

```rust
// Log every request
log::info!(
    "[TRANSFER] request_id={} from={} to={} amount={} status=requesting",
    request_id, from_account, to_account, amount
);

// Log status changes
log::info!(
    "[TRANSFER] request_id={} status={} -> {}",
    request_id, old_status, new_status
);

// Log errors with context
log::error!(
    "[TRANSFER] request_id={} error={} context={:?}",
    request_id, error_code, error_details
);
```

## Health Checks

```rust
// Endpoint: GET /health/internal_transfer
async fn health_check() -> Result<HealthStatus> {
    // Check DB connectivity
    db.get_transfer_by_id(1).await?;

    // Check TB connectivity (if applicable)
    tb_client.get_balance(test_account).await?;

    Ok(HealthStatus::Healthy)
}
```
