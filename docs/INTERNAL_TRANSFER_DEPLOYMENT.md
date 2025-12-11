# Internal Transfer - Production Deployment Guide

**Version**: 1.0
**Last Updated**: 2025-12-12
**Target Environment**: Production

---

## 🚀 Prerequisites

### Infrastructure Requirements
- **ScyllaDB Cluster**: 3+ nodes, RF=3
- **TigerBeetle Cluster**: 3+ replicas
- **Kafka/Redpanda**: 3+ brokers
- **Load Balancer**: Nginx or HAProxy
- **Monitoring**: Prometheus + Grafana

### System Requirements
- **CPU**: 8+ cores per service
- **Memory**: 16GB+ RAM per service
- **Disk**: NVMe SSD (for DB + TB)
- **Network**: 10Gbps+

---

## 📋 Deployment Checklist

### Phase 1: Database Setup

```bash
# 1. Start ScyllaDB cluster
docker-compose -f docker-compose.prod.yml up -d scylla1 scylla2 scylla3

# 2. Wait for cluster formation
nodetool status

# 3. Create keyspace
cqlsh -f schema/create_keyspace_prod.cql

# 4. Apply schema
cqlsh -f schema/internal_transfer.cql

# 5. Verify schema
cqlsh -e "DESCRIBE trading.balance_transfer_requests;"
```

### Phase 2: TigerBeetle Setup

```bash
# 1. Initialize cluster IDs
TB_CLUSTER_ID=1001

# 2. Format replicas
tigerbeetle format --cluster=$TB_CLUSTER_ID --replica=0 --replica-count=3 data_0
tigerbeetle format --cluster=$TB_CLUSTER_ID --replica=1 --replica-count=3 data_1
tigerbeetle format --cluster=$TB_CLUSTER_ID --replica=2 --replica-count=3 data_2

# 3. Start cluster
tigerbeetle start --cluster=$TB_CLUSTER_ID --replica=0 --addresses=3001,3002,3003 &
tigerbeetle start --cluster=$TB_CLUSTER_ID --replica=1 --addresses=3001,3002,3003 &
tigerbeetle start --cluster=$TB_CLUSTER_ID --replica=2 --addresses=3001,3002,3003 &

# 4. Verify cluster health
tigerbeetle status --addresses=3001,3002,3003
```

### Phase 3: Service Configuration

```yaml
# config/internal_transfer_prod.yaml
database:
  contact_points:
    - "scylla1.prod.internal:9042"
    - "scylla2.prod.internal:9042"
    - "scylla3.prod.internal:9042"
  keyspace: "trading"
  consistency: "QUORUM"
  replication_factor: 3

tigerbeetle:
  cluster_id: 1001
  addresses:
    - "tb1.prod.internal:3001"
    - "tb2.prod.internal:3002"
    - "tb3.prod.internal:3003"
  concurrency: 32

kafka:
  brokers:
    - "kafka1.prod.internal:9092"
    - "kafka2.prod.internal:9092"
    - "kafka3.prod.internal:9092"
  topic: "transfer_confirmations"
  consumer_group: "settlement_service"

rate_limiting:
  max_requests_per_minute: 60
  max_transfer_amount: 1000000000000000  # 10M USDT
  min_transfer_amount: 1000000           # 0.01 USDT

settlement:
  scanner_interval_ms: 5000
  stuck_threshold_ms: 1800000  # 30 minutes
  critical_threshold_ms: 7200000  # 2 hours

monitoring:
  prometheus_port: 9090
  health_check_port: 8080
```

### Phase 4: Build & Deploy Services

```bash
# 1. Build release binaries
cargo build --release

# 2. Create deployment package
tar -czf internal-transfer-v1.0.tar.gz \
    target/release/gateway_service \
    target/release/settlement_service \
    config/ \
    schema/

# 3. Deploy to servers
scp internal-transfer-v1.0.tar.gz prod-gateway-1:/opt/services/
scp internal-transfer-v1.0.tar.gz prod-settlement-1:/opt/services/

# 4. Extract on servers
ssh prod-gateway-1 "cd /opt/services && tar -xzf internal-transfer-v1.0.tar.gz"
ssh prod-settlement-1 "cd /opt/services && tar -xzf internal-transfer-v1.0.tar.gz"
```

### Phase 5: Service Startup

```bash
# On Gateway Server
systemctl start internal-transfer-gateway
systemctl enable internal-transfer-gateway
systemctl status internal-transfer-gateway

# On Settlement Server
systemctl start internal-transfer-settlement
systemctl enable internal-transfer-settlement
systemctl status internal-transfer-settlement
```

---

## 🔍 Verification Steps

### 1. Health Checks
```bash
# Gateway health
curl http://gateway:8080/health
# Expected: {"status": "healthy", "version": "1.0.0"}

# Settlement health
curl http://settlement:8080/health
# Expected: {"status": "healthy", "scanner": "running"}
```

### 2. Metrics
```bash
# Prometheus metrics
curl http://gateway:9090/metrics | grep internal_transfer
curl http://settlement:9090/metrics | grep internal_transfer
```

### 3. Test Transfer
```bash
# Create test transfer
curl -X POST http://gateway/api/v1/user/internal_transfer \
  -H "Authorization: Bearer $TEST_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {"account_type": "funding", "asset": "USDT"},
    "to_account": {"account_type": "spot", "user_id": 1000, "asset": "USDT"},
    "amount": "10.00000000"
  }'

# Query status
REQUEST_ID=$(echo $RESPONSE | jq -r '.data.request_id')
curl http://gateway/api/v1/user/internal_transfer/$REQUEST_ID
```

---

## 📊 Monitoring & Alerts

### Critical Alerts

```yaml
# alerts/internal_transfer.yml
groups:
  - name: internal_transfer
    interval: 30s
    rules:
      - alert: HighFailureRate
        expr: rate(internal_transfer_failed[5m]) / rate(internal_transfer_total_requests[5m]) > 0.05
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "Transfer failure rate > 5%"

      - alert: StuckTransfers
        expr: internal_transfer_stuck_count > 10
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "{{ $value }} transfers stuck > 30min"

      - alert: HighLatency
        expr: internal_transfer_avg_latency_ms > 100
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Average latency > 100ms"

      - alert: TigerBeetleDown
        expr: internal_transfer_tb_errors > 100
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "TigerBeetle errors detected"

      - alert: DatabaseDown
        expr: internal_transfer_db_errors > 100
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Database errors detected"
```

### Dashboards

```json
// Grafana Dashboard: Internal Transfer Overview
{
  "panels": [
    {
      "title": "Transfer Rate",
      "targets": ["rate(internal_transfer_total_requests[1m])"]
    },
    {
      "title": "Success Rate",
      "targets": ["internal_transfer_success_rate"]
    },
    {
      "title": "Latency P50/P99",
      "targets": [
        "histogram_quantile(0.50, internal_transfer_latency_ms)",
        "histogram_quantile(0.99, internal_transfer_latency_ms)"
      ]
    },
    {
      "title": "Pending Transfers",
      "targets": ["internal_transfer_pending_count"]
    },
    {
      "title": "Stuck Transfers",
      "targets": ["internal_transfer_stuck_count"]
    }
  ]
}
```

---

## 🔧 Maintenance

### Daily Tasks
- [ ] Check metrics dashboard
- [ ] Review stuck transfer reports
- [ ] Verify settlement scanner logs
- [ ] Check error rates

### Weekly Tasks
- [ ] Analyze performance trends
- [ ] Review capacity planning
- [ ] Check DB disk usage
- [ ] Backup configurations

### Monthly Tasks
- [ ] Performance optimization review
- [ ] Security audit
- [ ] Disaster recovery drill
- [ ] Documentation updates

---

## 🚨 Incident Response

### High Failure Rate
1. Check TigerBeetle cluster health
2. Check database connection pool
3. Review recent code deployments
4. Enable detailed logging
5. Consider rollback if recent deploy

### Stuck Transfers
1. Check settlement service logs
2. Verify Kafka consumer lag
3. Run manual reconciliation
4. Check TigerBeetle replica sync
5. Manual intervention if needed

### Database Issues
1. Check ScyllaDB cluster status
2. Review slow query logs
3. Check disk I/O metrics
4. Verify replication factor
5. Consider adding nodes

---

## 📝 Rollback Procedure

```bash
# 1. Stop new services
systemctl stop internal-transfer-gateway
systemctl stop internal-transfer-settlement

# 2. Restore previous version
cd /opt/services/backups
tar -xzf internal-transfer-v0.9.tar.gz

# 3. Start old services
systemctl start internal-transfer-gateway-old
systemctl start internal-transfer-settlement-old

# 4. Verify functionality
curl http://gateway:8080/health

# 5. Monitor for 30 minutes
watch -n 10 'curl -s http://gateway:9090/metrics | grep success_rate'
```

---

## ✅ Production Readiness Checklist

### Code Quality
- [x] All unit tests passing
- [x] Integration tests passing
- [x] Load tests completed (5K TPS)
- [x] Security audit completed
- [x] Code review approved

### Infrastructure
- [ ] ScyllaDB cluster operational
- [ ] TigerBeetle cluster operational
- [ ] Kafka cluster operational
- [ ] Load balancers configured
- [ ] Monitoring dashboards created

### Operations
- [ ] Runbooks created
- [ ] Alerts configured
- [ ] On-call rotation setup
- [ ] Incident response plan documented
- [ ] Disaster recovery tested

### Documentation
- [x] API documentation complete
- [x] Deployment guide complete
- [x] Monitoring guide complete
- [x] Architecture diagrams created
- [x] Troubleshooting guide created

---

**Deployment Owner**: DevOps Team
**Approval Required**: Engineering Manager + SRE Lead
**Go-Live Date**: TBD
**Rollback Plan**: Documented above
