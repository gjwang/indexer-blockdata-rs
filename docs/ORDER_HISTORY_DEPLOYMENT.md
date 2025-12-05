# Order History Service - Production Deployment Guide

## 📋 Overview

This guide covers the complete deployment process for the Order History Service in a production environment.

---

## 🔧 Prerequisites

### System Requirements
- **OS**: Linux (Ubuntu 20.04+ recommended) or macOS
- **CPU**: 4+ cores recommended
- **RAM**: 8GB+ recommended
- **Disk**: 100GB+ for ScyllaDB data

### Software Requirements
- **Rust**: 1.70+ (`rustc --version`)
- **ScyllaDB**: 5.0+ or Cassandra 4.0+
- **ZMQ**: libzmq 4.3+
- **cqlsh**: For schema management

---

## 📦 Installation

### 1. Install Dependencies

**Ubuntu/Debian**:
```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# ZMQ
sudo apt-get update
sudo apt-get install -y libzmq3-dev pkg-config

# ScyllaDB tools
sudo apt-get install -y scylla-tools
```

**macOS**:
```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# ZMQ
brew install zmq pkg-config

# ScyllaDB tools
brew install scylla-tools
```

### 2. Clone Repository
```bash
git clone <repository-url>
cd indexer-blockdata-rs
```

### 3. Build Release Binary
```bash
cargo build --release --bin order_history_service

# Verify build
ls -lh target/release/order_history_service
```

---

## 🗄️ Database Setup

### 1. Start ScyllaDB

**Docker (Development)**:
```bash
docker-compose up -d scylla

# Wait for ScyllaDB to be ready
docker logs -f scylla | grep "Starting listening for CQL clients"
```

**Production (Systemd)**:
```bash
sudo systemctl start scylla-server
sudo systemctl enable scylla-server
sudo systemctl status scylla-server
```

### 2. Initialize Schema
```bash
./scripts/init_order_history_schema.sh

# Verify tables
cqlsh -k trading -e "DESCRIBE TABLES;"
```

**Expected Output**:
```
active_orders  order_history  order_statistics  order_updates_stream
```

### 3. Verify Indexes
```bash
cqlsh -k trading -e "DESCRIBE INDEX idx_active_orders_symbol;"
cqlsh -k trading -e "DESCRIBE INDEX idx_order_history_order_id;"
```

---

## ⚙️ Configuration

### 1. Create Configuration File

**Production Config** (`config/order_history_config.yaml`):
```yaml
# Logging
log_file: "/var/log/trading/order_history_service.log"
log_level: "info"
log_to_file: true

# Data Directory
data_dir: "/var/lib/trading/data"

# ZeroMQ Configuration
zeromq:
  settlement_port: 5556  # Shared with settlement service

# ScyllaDB Configuration
scylladb:
  hosts:
    - "scylla-node-1:9042"
    - "scylla-node-2:9042"
    - "scylla-node-3:9042"
  keyspace: "trading"
  username: ""
  password: ""
```

### 2. Create Log Directory
```bash
sudo mkdir -p /var/log/trading
sudo chown $USER:$USER /var/log/trading
```

### 3. Create Data Directory
```bash
sudo mkdir -p /var/lib/trading/data
sudo chown $USER:$USER /var/lib/trading/data
```

---

## 🚀 Deployment

### Option 1: Systemd Service (Recommended)

**1. Create Service File** (`/etc/systemd/system/order-history.service`):
```ini
[Unit]
Description=Order History Service
After=network.target scylla-server.service
Requires=scylla-server.service

[Service]
Type=simple
User=trading
Group=trading
WorkingDirectory=/opt/trading/indexer-blockdata-rs
ExecStart=/opt/trading/indexer-blockdata-rs/target/release/order_history_service
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal

# Resource limits
LimitNOFILE=65536
LimitNPROC=4096

# Environment
Environment="RUST_LOG=info"
Environment="RUST_BACKTRACE=1"

[Install]
WantedBy=multi-user.target
```

**2. Install and Start**:
```bash
# Copy binary to production location
sudo mkdir -p /opt/trading/indexer-blockdata-rs
sudo cp -r . /opt/trading/indexer-blockdata-rs/

# Create trading user
sudo useradd -r -s /bin/false trading

# Set permissions
sudo chown -R trading:trading /opt/trading/indexer-blockdata-rs

# Reload systemd
sudo systemctl daemon-reload

# Enable and start service
sudo systemctl enable order-history
sudo systemctl start order-history

# Check status
sudo systemctl status order-history
```

**3. View Logs**:
```bash
# Systemd journal
sudo journalctl -u order-history -f

# Application log
tail -f /var/log/trading/order_history_service.log
```

### Option 2: Docker Container

**1. Create Dockerfile**:
```dockerfile
FROM rust:1.75 as builder

WORKDIR /app
COPY . .

RUN cargo build --release --bin order_history_service

FROM ubuntu:22.04

RUN apt-get update && apt-get install -y \
    libzmq5 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/order_history_service /usr/local/bin/
COPY config/order_history_config.yaml /etc/trading/

EXPOSE 5556

CMD ["order_history_service"]
```

**2. Build and Run**:
```bash
# Build image
docker build -t order-history-service:latest .

# Run container
docker run -d \
  --name order-history \
  --network trading-network \
  -v /var/log/trading:/var/log/trading \
  -v /var/lib/trading:/var/lib/trading \
  --restart unless-stopped \
  order-history-service:latest
```

### Option 3: Manual Process (Development)

```bash
# Run in foreground
./target/release/order_history_service

# Run in background
nohup ./target/release/order_history_service > logs/order_history.log 2>&1 &

# Save PID
echo $! > /var/run/order_history.pid
```

---

## 🔍 Monitoring

### Health Checks

**1. Service Status**:
```bash
# Systemd
sudo systemctl status order-history

# Process
ps aux | grep order_history_service

# Port listening
netstat -tlnp | grep 5556
```

**2. Database Connection**:
```bash
# Check ScyllaDB health
cqlsh -e "SELECT now() FROM system.local;"

# Check table counts
cqlsh -k trading -e "SELECT COUNT(*) FROM active_orders;"
cqlsh -k trading -e "SELECT COUNT(*) FROM order_history;"
```

**3. Log Monitoring**:
```bash
# Check for errors
grep "ERROR" /var/log/trading/order_history_service.log

# Check for warnings
grep "WARN" /var/log/trading/order_history_service.log

# Monitor statistics
grep "Statistics" /var/log/trading/order_history_service.log | tail -10
```

### Metrics (Optional)

**Prometheus Metrics** (Future Enhancement):
```yaml
# /etc/prometheus/prometheus.yml
scrape_configs:
  - job_name: 'order-history'
    static_configs:
      - targets: ['localhost:9090']
```

---

## 🔧 Maintenance

### Log Rotation

**Create logrotate config** (`/etc/logrotate.d/order-history`):
```
/var/log/trading/order_history_service.log {
    daily
    rotate 30
    compress
    delaycompress
    missingok
    notifempty
    create 0644 trading trading
    postrotate
        systemctl reload order-history > /dev/null 2>&1 || true
    endscript
}
```

### Database Maintenance

**1. Compaction**:
```bash
# Manual compaction
nodetool compact trading active_orders
nodetool compact trading order_history
```

**2. Repair** (if using replication):
```bash
nodetool repair trading
```

**3. Cleanup Old Data**:
```sql
-- Delete old order_updates_stream (older than 90 days)
DELETE FROM order_updates_stream
WHERE event_date < 20231201;
```

---

## 🚨 Troubleshooting

### Service Won't Start

**Check logs**:
```bash
sudo journalctl -u order-history -n 100 --no-pager
```

**Common issues**:
1. **ScyllaDB not running**: `sudo systemctl start scylla-server`
2. **Port already in use**: `lsof -i :5556`
3. **Permission denied**: Check file permissions
4. **Config file missing**: Verify `/etc/trading/order_history_config.yaml`

### No Events Received

**Check ZMQ connection**:
```bash
# Verify Matching Engine is publishing
netstat -an | grep 5556

# Check firewall
sudo ufw status
```

### Database Connection Errors

**Check ScyllaDB**:
```bash
# Service status
sudo systemctl status scylla-server

# Connection test
cqlsh -e "SELECT now() FROM system.local;"

# Check keyspace
cqlsh -e "DESCRIBE KEYSPACE trading;"
```

---

## 📊 Performance Tuning

### ScyllaDB Optimization

**1. Compaction Strategy**:
```sql
ALTER TABLE active_orders
WITH compaction = {'class': 'LeveledCompactionStrategy'};

ALTER TABLE order_history
WITH compaction = {'class': 'TimeWindowCompactionStrategy',
                   'compaction_window_unit': 'DAYS',
                   'compaction_window_size': 1};
```

**2. Read/Write Consistency**:
```yaml
# In application code
consistency_level: LOCAL_QUORUM  # For production with replication
```

### Service Optimization

**1. Increase ZMQ Buffer**:
```yaml
# config/order_history_config.yaml
zeromq:
  settlement_port: 5556
  rcvhwm: 10000000  # Increase high water mark
```

**2. Batch Writes** (Future Enhancement):
```rust
// Batch multiple inserts for better performance
let batch = BatchStatement::new();
batch.append_statement(insert_active_order);
batch.append_statement(insert_order_history);
session.batch(&batch, &values).await?;
```

---

## 🔐 Security

### Network Security

**1. Firewall Rules**:
```bash
# Allow ZMQ port (internal only)
sudo ufw allow from 10.0.0.0/8 to any port 5556

# Allow ScyllaDB (internal only)
sudo ufw allow from 10.0.0.0/8 to any port 9042
```

**2. TLS/SSL** (Recommended for production):
```yaml
# ScyllaDB with TLS
scylladb:
  hosts: ["scylla-node-1:9042"]
  ssl:
    enabled: true
    ca_cert: "/etc/ssl/certs/ca.pem"
```

---

## ✅ Production Checklist

- [ ] ScyllaDB cluster deployed and healthy
- [ ] Schema initialized with all tables and indexes
- [ ] Configuration file created and validated
- [ ] Service binary built in release mode
- [ ] Systemd service file created
- [ ] Log rotation configured
- [ ] Monitoring and alerting set up
- [ ] Firewall rules configured
- [ ] Backup strategy implemented
- [ ] Disaster recovery plan documented
- [ ] Load testing completed
- [ ] Security audit performed

---

## 📞 Support

For issues or questions:
1. Check logs: `/var/log/trading/order_history_service.log`
2. Review documentation: `docs/ORDER_HISTORY_FINAL_SUMMARY.md`
3. Run E2E test: `./scripts/test_order_history_e2e.sh`

---

**Last Updated**: 2025-12-05
**Version**: 1.0.0
**Status**: Production Ready ✅
