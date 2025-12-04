# Development Environment Guide

## 🐳 Docker-First Development Philosophy

**CRITICAL RULE**: All dependency services MUST run in Docker containers in local development.

### Why Docker for Dependencies?

1. **No System Pollution**: Your Mac stays clean - no databases, message brokers, or services installed directly
2. **Version Consistency**: Everyone uses the exact same service versions
3. **Easy Reset**: `docker-compose down -v` gives you a fresh start
4. **Production Parity**: Development environment matches production
5. **Onboarding Speed**: New developers can start in minutes

### ❌ DO NOT Install These Locally

- ❌ Kafka/Redpanda via Homebrew
- ❌ ScyllaDB/Cassandra via Homebrew
- ❌ Centrifugo binary
- ❌ PostgreSQL (if we add it later)
- ❌ Redis (if we add it later)

### ✅ DO Use Docker

```bash
# This is the ONLY way to run dependencies
docker-compose up -d
```

---

## Docker Compose Services

### Current Services

#### 1. Redpanda (Kafka-compatible)
```yaml
Service: redpanda
Image: redpandadata/redpanda:latest
Ports:
  - 9093: Kafka API (external)
  - 29092: Kafka API (internal)
  - 8081: Schema Registry
  - 8082: HTTP Proxy
  - 9644: Admin API
```

**Usage:**
```bash
# Start
docker-compose up -d redpanda

# Check logs
docker logs redpanda -f

# List topics
docker exec redpanda rpk topic list

# Create topic
docker exec redpanda rpk topic create my-topic

# Consume messages
docker exec redpanda rpk topic consume my-topic
```

#### 2. ScyllaDB (Cassandra-compatible)
```yaml
Service: scylla
Image: scylladb/scylla:5.4
Ports:
  - 9042: CQL native transport
  - 9160: Thrift (legacy)
  - 10000: REST API
Resources:
  - Memory: 512MB
  - CPU: 1 core
```

**Usage:**
```bash
# Start
docker-compose up -d scylla

# Wait for initialization (30-60 seconds)
docker logs scylla | grep "initialization completed"

# Initialize schema
./scripts/init_scylla.sh

# Connect with cqlsh
docker exec -it scylla cqlsh

# Run CQL command
docker exec scylla cqlsh -e "DESCRIBE KEYSPACES"

# Query data
docker exec scylla cqlsh -e "SELECT * FROM settlement.settled_trades LIMIT 10"
```

---

## Common Docker Commands

### Service Management

```bash
# Start all services
docker-compose up -d

# Start specific service
docker-compose up -d scylla

# Stop all services (keeps data)
docker-compose stop

# Stop specific service
docker-compose stop scylla

# Restart service
docker-compose restart scylla

# Remove services (keeps data)
docker-compose down

# Remove services AND data
docker-compose down -v
```

### Monitoring

```bash
# List running containers
docker-compose ps
docker ps

# View logs (follow mode)
docker logs scylla -f
docker logs redpanda -f

# View last N lines
docker logs scylla --tail 50

# View logs since timestamp
docker logs scylla --since 5m

# Check resource usage
docker stats
```

### Debugging

```bash
# Execute command in container
docker exec scylla cqlsh -e "DESCRIBE KEYSPACES"
docker exec redpanda rpk topic list

# Interactive shell
docker exec -it scylla bash
docker exec -it redpanda bash

# Inspect container
docker inspect scylla

# View container config
docker-compose config
```

### Data Management

```bash
# List volumes
docker volume ls

# Inspect volume
docker volume inspect indexer-blockdata-rs_scylla-data

# Remove specific volume
docker volume rm indexer-blockdata-rs_scylla-data

# Remove all unused volumes
docker volume prune
```

---

## Development Workflow

### Daily Workflow

```bash
# 1. Start services (if not running)
docker-compose up -d

# 2. Check services are healthy
docker-compose ps

# 3. Develop and test
cargo run --bin matching_engine_server

# 4. Stop services when done (optional)
docker-compose stop
```

### Fresh Start Workflow

```bash
# 1. Stop and remove everything
docker-compose down -v

# 2. Start fresh
docker-compose up -d

# 3. Wait for initialization
sleep 30

# 4. Initialize schemas
./scripts/init_scylla.sh

# 5. Run your service
cargo run --bin settlement_service
```

### Adding a New Service

1. **Update `docker-compose.yml`**:
   ```yaml
   my-service:
     image: my-service:latest
     ports:
       - "8080:8080"
     volumes:
       - my-data:/data
   ```

2. **Add to volumes section**:
   ```yaml
   volumes:
     my-data:
   ```

3. **Create initialization script** (if needed):
   ```bash
   scripts/init_my_service.sh
   ```

4. **Update README.md** with service info

5. **Test**:
   ```bash
   docker-compose up -d my-service
   docker logs my-service
   ```

---

## Troubleshooting

### Service Won't Start

```bash
# Check logs
docker logs scylla

# Check if port is already in use
lsof -i :9042

# Remove and recreate
docker-compose down
docker-compose up -d scylla
```

### Out of Disk Space

```bash
# Check Docker disk usage
docker system df

# Clean up unused images
docker image prune -a

# Clean up unused volumes
docker volume prune

# Clean up everything
docker system prune -a --volumes
```

### Service Stuck Initializing

```bash
# For ScyllaDB (can take 60+ seconds first time)
docker logs scylla -f

# If stuck, restart
docker-compose restart scylla

# If still stuck, recreate
docker-compose down
docker-compose up -d scylla
```

### Can't Connect to Service

```bash
# Check service is running
docker-compose ps

# Check port mapping
docker port scylla

# Test connection from host
nc -zv localhost 9042

# Test from inside container
docker exec scylla cqlsh -e "SELECT now() FROM system.local"
```

### Data Corruption

```bash
# Nuclear option: delete everything and start fresh
docker-compose down -v
docker volume rm indexer-blockdata-rs_scylla-data
docker volume rm indexer-blockdata-rs_redpanda-data
docker-compose up -d
./scripts/init_scylla.sh
```

---

## Performance Tuning

### ScyllaDB

```yaml
# In docker-compose.yml
scylla:
  command: >
    --smp 2                    # Use 2 CPU cores
    --memory 1G                # Allocate 1GB RAM
    --overprovisioned 1        # Run on overprovisioned hardware
```

### Redpanda

```yaml
# In docker-compose.yml
redpanda:
  command:
    - --memory=1G              # Allocate 1GB RAM
    - --smp=2                  # Use 2 CPU cores
```

---

## Best Practices

### ✅ DO

- Use `docker-compose up -d` to start services
- Check logs with `docker logs <service>`
- Use volumes for persistent data
- Keep `docker-compose.yml` in version control
- Document service versions
- Use health checks
- Clean up regularly with `docker system prune`

### ❌ DON'T

- Install services directly on your Mac
- Modify running containers (changes are lost on restart)
- Use `latest` tag in production (pin versions)
- Commit sensitive data to docker-compose.yml
- Run services without resource limits
- Forget to initialize schemas

---

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Test
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      
      - name: Start services
        run: docker-compose up -d
      
      - name: Wait for services
        run: sleep 30
      
      - name: Initialize
        run: ./scripts/init_scylla.sh
      
      - name: Run tests
        run: cargo test
      
      - name: Cleanup
        run: docker-compose down -v
```

---

## Additional Resources

- [Docker Compose Documentation](https://docs.docker.com/compose/)
- [ScyllaDB Docker Guide](https://hub.docker.com/r/scylladb/scylla)
- [Redpanda Docker Guide](https://docs.redpanda.com/docs/get-started/quick-start/)

---

## Questions?

If you encounter issues not covered here:
1. Check `docker logs <service>`
2. Try `docker-compose down -v` and start fresh
3. Check Docker Desktop is running
4. Verify ports aren't in use: `lsof -i :<port>`
