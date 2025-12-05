# Order History Service - Monitoring & Observability Guide

## 📊 Overview

This guide covers monitoring, logging, metrics, and observability for the Order History Service in production.

---

## 📝 Logging

### Log Levels

The service uses structured logging with the following levels:

| Level | Usage | Example |
|-------|-------|---------|
| `ERROR` | Critical failures | Database connection lost |
| `WARN` | Recoverable issues | Retry attempts, degraded performance |
| `INFO` | Normal operations | Service started, events processed |
| `DEBUG` | Detailed diagnostics | Event details, query parameters |
| `TRACE` | Very verbose | Internal state changes |

### Log Format

```
[2025-12-05T23:26:32Z INFO  order_history] 📋 OrderUpdate #1 | Order #101 | User 1 | BTC_USDT | 🆕 NEW | Price: 50000 | Qty: 1 | Filled: 0
```

**Fields**:
- Timestamp (UTC)
- Log level
- Target (module name)
- Message with emoji indicators

### Log Locations

**Development**:
```
logs/order_history_service.log
```

**Production**:
```
/var/log/trading/order_history_service.log
```

**Systemd Journal**:
```bash
sudo journalctl -u order-history -f
```

### Log Queries

**Find errors**:
```bash
grep "ERROR" /var/log/trading/order_history_service.log
```

**Find specific order**:
```bash
grep "Order #101" /var/log/trading/order_history_service.log
```

**Count events by status**:
```bash
grep "OrderUpdate" /var/log/trading/order_history_service.log | \
  grep -oP '(NEW|FILLED|CANCELLED|REJECTED)' | \
  sort | uniq -c
```

**Statistics summary**:
```bash
grep "Statistics" /var/log/trading/order_history_service.log | tail -1
```

---

## 📈 Metrics

### Service Metrics

**Event Processing**:
- `total_events` - Total OrderUpdate events received
- `total_persisted` - Successfully persisted events
- `total_errors` - Failed persistence attempts
- `events_by_status` - Count by OrderStatus

**Example Log Output**:
```
📊 Statistics:
   Total Events: 1000
   Persisted: 998
   Errors: 2
   New: 450
   Filled: 300
   Cancelled: 200
   Rejected: 50
```

### Database Metrics

**Query ScyllaDB**:
```sql
-- Active orders count
SELECT COUNT(*) FROM active_orders;

-- Order history count
SELECT COUNT(*) FROM order_history;

-- Events by date
SELECT event_date, COUNT(*)
FROM order_updates_stream
GROUP BY event_date;

-- User statistics
SELECT * FROM order_statistics WHERE user_id = 1;
```

### System Metrics

**CPU & Memory**:
```bash
# Process stats
ps aux | grep order_history_service

# Detailed stats
top -p $(pgrep order_history_service)
```

**Network**:
```bash
# ZMQ connections
netstat -an | grep 5556

# ScyllaDB connections
netstat -an | grep 9042
```

---

## 🔍 Health Checks

### Service Health

**1. Process Check**:
```bash
#!/bin/bash
# health_check.sh

if pgrep -f order_history_service > /dev/null; then
    echo "✅ Service is running"
    exit 0
else
    echo "❌ Service is NOT running"
    exit 1
fi
```

**2. Log Check**:
```bash
#!/bin/bash
# Check for recent activity (last 60 seconds)

LAST_LOG=$(tail -1 /var/log/trading/order_history_service.log | \
           grep -oP '\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}')

if [ -z "$LAST_LOG" ]; then
    echo "❌ No recent logs"
    exit 1
fi

LAST_TIMESTAMP=$(date -d "$LAST_LOG" +%s)
NOW=$(date +%s)
DIFF=$((NOW - LAST_TIMESTAMP))

if [ $DIFF -lt 60 ]; then
    echo "✅ Service is active (last log: ${DIFF}s ago)"
    exit 0
else
    echo "⚠️  Service may be stuck (last log: ${DIFF}s ago)"
    exit 1
fi
```

**3. Database Health**:
```bash
#!/bin/bash
# db_health_check.sh

if cqlsh -e "SELECT COUNT(*) FROM trading.active_orders;" > /dev/null 2>&1; then
    echo "✅ Database connection OK"
    exit 0
else
    echo "❌ Database connection FAILED"
    exit 1
fi
```

### Automated Health Monitoring

**Cron Job** (`/etc/cron.d/order-history-health`):
```bash
*/5 * * * * root /opt/trading/scripts/health_check.sh || systemctl restart order-history
```

---

## 🚨 Alerting

### Alert Conditions

**Critical Alerts** (Page immediately):
1. Service down for > 5 minutes
2. Database connection lost
3. Error rate > 5% of total events
4. No events received for > 10 minutes (during trading hours)

**Warning Alerts** (Email/Slack):
1. Error rate > 1% of total events
2. Persistence latency > 1 second
3. Disk usage > 80%
4. Memory usage > 80%

### Alert Script Example

```bash
#!/bin/bash
# alert_on_errors.sh

ERROR_COUNT=$(grep "ERROR" /var/log/trading/order_history_service.log | \
              grep "$(date +%Y-%m-%d)" | wc -l)

TOTAL_EVENTS=$(grep "OrderUpdate" /var/log/trading/order_history_service.log | \
               grep "$(date +%Y-%m-%d)" | wc -l)

if [ $TOTAL_EVENTS -gt 0 ]; then
    ERROR_RATE=$(echo "scale=2; $ERROR_COUNT * 100 / $TOTAL_EVENTS" | bc)

    if (( $(echo "$ERROR_RATE > 5.0" | bc -l) )); then
        echo "🚨 CRITICAL: Error rate is ${ERROR_RATE}%"
        # Send alert (email, Slack, PagerDuty, etc.)
        curl -X POST https://hooks.slack.com/services/YOUR/WEBHOOK/URL \
          -H 'Content-Type: application/json' \
          -d "{\"text\":\"🚨 Order History Service: Error rate ${ERROR_RATE}%\"}"
    fi
fi
```

---

## 📊 Dashboards

### Grafana Dashboard (Example)

**Panels**:

1. **Event Processing Rate**
   - Query: Events per minute
   - Visualization: Time series graph

2. **Error Rate**
   - Query: Errors / Total events
   - Visualization: Gauge (0-100%)

3. **Active Orders Count**
   - Query: `SELECT COUNT(*) FROM active_orders`
   - Visualization: Stat panel

4. **Order History Growth**
   - Query: `SELECT COUNT(*) FROM order_history`
   - Visualization: Time series graph

5. **Events by Status**
   - Query: Count by OrderStatus
   - Visualization: Pie chart

6. **Database Latency**
   - Query: Query execution time
   - Visualization: Heatmap

### Sample Prometheus Queries

```promql
# Event processing rate
rate(order_history_events_total[5m])

# Error rate
rate(order_history_errors_total[5m]) / rate(order_history_events_total[5m])

# Active orders
order_history_active_orders_count

# Database query latency (p99)
histogram_quantile(0.99, rate(order_history_db_query_duration_seconds_bucket[5m]))
```

---

## 🔎 Debugging

### Common Issues

**1. No Events Received**

**Check**:
```bash
# Is Matching Engine running?
ps aux | grep matching_engine_server

# Is ZMQ port open?
netstat -an | grep 5556

# Check service logs
grep "Waiting for OrderUpdate events" logs/order_history_service.log
```

**2. Database Errors**

**Check**:
```bash
# ScyllaDB status
sudo systemctl status scylla-server

# Connection test
cqlsh -e "SELECT now() FROM system.local;"

# Check service logs
grep "ScyllaDB" logs/order_history_service.log
```

**3. High Error Rate**

**Check**:
```bash
# View recent errors
grep "ERROR" logs/order_history_service.log | tail -20

# Check error types
grep "ERROR" logs/order_history_service.log | \
  grep -oP 'ERROR.*' | sort | uniq -c | sort -rn
```

### Debug Mode

**Enable debug logging**:
```yaml
# config/order_history_config.yaml
log_level: "debug"
```

**Restart service**:
```bash
sudo systemctl restart order-history
```

**View debug logs**:
```bash
tail -f /var/log/trading/order_history_service.log | grep DEBUG
```

---

## 📉 Performance Monitoring

### Latency Tracking

**Add timing logs** (Future Enhancement):
```rust
let start = Instant::now();
db.insert_order_history(&order_update).await?;
let duration = start.elapsed();

if duration.as_millis() > 100 {
    log::warn!("Slow database write: {}ms", duration.as_millis());
}
```

### Throughput Monitoring

**Events per second**:
```bash
# Count events in last minute
grep "OrderUpdate" logs/order_history_service.log | \
  grep "$(date -d '1 minute ago' '+%Y-%m-%d %H:%M')" | wc -l
```

**Database write rate**:
```bash
# Monitor ScyllaDB writes
nodetool tablestats trading.order_history | grep "Write Count"
```

---

## 🎯 SLI/SLO Recommendations

### Service Level Indicators (SLIs)

1. **Availability**: Service uptime
   - Target: 99.9% (43 minutes downtime/month)

2. **Event Processing Success Rate**: Successful persistence / Total events
   - Target: 99.99% (1 error per 10,000 events)

3. **Latency**: Time from event receipt to database persistence
   - Target: p99 < 100ms

4. **Data Freshness**: Time lag between ME and Order History
   - Target: < 1 second

### Service Level Objectives (SLOs)

**Monthly Targets**:
- Availability: 99.9%
- Success Rate: 99.99%
- p99 Latency: < 100ms
- Data Freshness: < 1s

**Error Budget**:
- Downtime: 43 minutes/month
- Failed Events: 0.01% of total

---

## 📋 Monitoring Checklist

Daily:
- [ ] Check service status
- [ ] Review error logs
- [ ] Verify event processing rate
- [ ] Check database connection

Weekly:
- [ ] Review performance metrics
- [ ] Check disk usage
- [ ] Analyze error trends
- [ ] Verify backup completion

Monthly:
- [ ] Review SLO compliance
- [ ] Analyze capacity trends
- [ ] Update alert thresholds
- [ ] Performance tuning review

---

## 🔗 Integration with Monitoring Tools

### Prometheus (Future)

**Metrics Endpoint**:
```rust
// Add to service
use prometheus::{Encoder, TextEncoder, Counter, Histogram};

lazy_static! {
    static ref EVENTS_TOTAL: Counter =
        Counter::new("order_history_events_total", "Total events").unwrap();

    static ref DB_LATENCY: Histogram =
        Histogram::new("order_history_db_latency_seconds", "DB latency").unwrap();
}
```

### Grafana Loki (Logs)

**Promtail Config**:
```yaml
scrape_configs:
  - job_name: order-history
    static_configs:
      - targets:
          - localhost
        labels:
          job: order-history
          __path__: /var/log/trading/order_history_service.log
```

### Datadog (APM)

**Tracing**:
```rust
use ddtrace::{Tracer, Span};

let tracer = Tracer::new();
let span = tracer.span("process_order_update");
// ... processing ...
span.finish();
```

---

**Last Updated**: 2025-12-05
**Version**: 1.0.0
**Status**: Production Ready ✅
