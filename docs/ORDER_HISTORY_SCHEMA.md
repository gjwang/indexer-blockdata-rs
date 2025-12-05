# Order History Database Schema

## Overview

The Order History Service uses 4 ScyllaDB tables to provide complete order lifecycle tracking with high-performance queries.

## Tables

### 1. `active_orders` (Open Orders)

**Purpose**: Fast lookup of currently open orders

**Partition Key**: `user_id`
**Clustering Key**: `order_id DESC`

```sql
CREATE TABLE active_orders (
    user_id bigint,
    order_id bigint,
    client_order_id text,
    symbol text,
    side text,
    order_type text,
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    status text,
    created_at bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, order_id)
);
```

**Query Patterns**:
- Get user's active orders: `SELECT * FROM active_orders WHERE user_id = ?`
- Get specific active order: `SELECT * FROM active_orders WHERE user_id = ? AND order_id = ?`

**Lifecycle**:
- **Insert**: When `OrderUpdate(New)` received
- **Update**: When `OrderUpdate(PartiallyFilled)` received
- **Delete**: When `OrderUpdate(Filled)` or `OrderUpdate(Cancelled)` received

### 2. `order_history` (Complete Audit Trail)

**Purpose**: Complete order history with time-based ordering

**Partition Key**: `user_id`
**Clustering Keys**: `created_at DESC`, `order_id DESC`

```sql
CREATE TABLE order_history (
    user_id bigint,
    created_at bigint,
    order_id bigint,
    client_order_id text,
    symbol text,
    side text,
    order_type text,
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    status text,
    rejection_reason text,
    match_id bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, created_at, order_id)
);
```

**Query Patterns**:
- Get user's recent orders: `SELECT * FROM order_history WHERE user_id = ? LIMIT 100`
- Get orders in time range: `SELECT * FROM order_history WHERE user_id = ? AND created_at >= ? AND created_at <= ?`
- Get specific order: `SELECT * FROM order_history WHERE order_id = ?` (via index)

**Lifecycle**:
- **Insert**: On EVERY `OrderUpdate` event (New, PartiallyFilled, Filled, Cancelled, Rejected)

### 3. `order_updates_stream` (Event Sourcing)

**Purpose**: Complete event stream for replay and audit

**Partition Key**: `event_date` (YYYYMMDD)
**Clustering Key**: `event_id DESC`

```sql
CREATE TABLE order_updates_stream (
    event_date int,
    event_id bigint,
    order_id bigint,
    user_id bigint,
    client_order_id text,
    symbol text,
    status text,
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    rejection_reason text,
    match_id bigint,
    timestamp bigint,
    PRIMARY KEY (event_date, event_id)
);
```

**Query Patterns**:
- Replay events for date: `SELECT * FROM order_updates_stream WHERE event_date = ?`
- Get events for order: `SELECT * FROM order_updates_stream WHERE order_id = ?` (via index)

**Lifecycle**:
- **Insert**: On EVERY `OrderUpdate` event

### 4. `order_statistics` (Aggregated Metrics)

**Purpose**: Per-user order statistics for analytics

**Partition Key**: `user_id`

```sql
CREATE TABLE order_statistics (
    user_id bigint PRIMARY KEY,
    total_orders bigint,
    filled_orders bigint,
    cancelled_orders bigint,
    rejected_orders bigint,
    total_volume bigint,
    last_order_at bigint,
    updated_at bigint
);
```

**Query Patterns**:
- Get user stats: `SELECT * FROM order_statistics WHERE user_id = ?`

**Lifecycle**:
- **Upsert**: On EVERY `OrderUpdate` event (increment counters)

## Indexes

| Table | Index | Purpose |
|-------|-------|---------|
| `active_orders` | `idx_active_orders_symbol` | Query by symbol |
| `order_history` | `idx_order_history_symbol` | Query by symbol |
| `order_history` | `idx_order_history_status` | Query by status |
| `order_history` | `idx_order_history_order_id` | Direct order lookup |
| `order_updates_stream` | `idx_updates_order_id` | Query by order |
| `order_updates_stream` | `idx_updates_user_id` | Query by user |

## Data Flow

```
OrderUpdate Event
    ↓
┌───────────────────────────────────┐
│  Order History Service            │
├───────────────────────────────────┤
│  1. Parse OrderUpdate             │
│  2. Match on status:              │
│     - New → Insert active_orders  │
│     - PartiallyFilled → Update    │
│     - Filled → Delete active      │
│     - Cancelled → Delete active   │
│     - Rejected → (no active)      │
│  3. Insert order_history (always) │
│  4. Insert order_updates_stream   │
│  5. Update order_statistics       │
└───────────────────────────────────┘
```

## Initialization

```bash
# Run initialization script
./scripts/init_order_history_schema.sh

# Or manually with cqlsh
cqlsh -k trading -f schema/order_history_schema.cql
```

## Performance Characteristics

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Get active orders | O(1) | Single partition read |
| Get order history | O(log n) | Time-ordered clustering |
| Insert order update | O(1) | 4 parallel writes |
| Query by order_id | O(log n) | Secondary index |
| Replay events | O(n) | Sequential scan per day |

## Storage Estimates

Assuming:
- 1M orders/day
- Average order size: 200 bytes
- Retention: 90 days

| Table | Daily Growth | 90-Day Total |
|-------|--------------|--------------|
| `active_orders` | ~20 MB | ~200 MB (steady state) |
| `order_history` | ~200 MB | ~18 GB |
| `order_updates_stream` | ~200 MB | ~18 GB |
| `order_statistics` | ~1 MB | ~1 MB (steady state) |
| **Total** | **~421 MB/day** | **~36 GB** |

## Backup & Recovery

- **Point-in-Time Recovery**: Use `order_updates_stream` to replay events
- **Snapshot**: ScyllaDB native snapshots
- **Replication**: Configure replication factor based on requirements

## Next Steps

- ✅ Schema created
- 🔄 Implement service to consume OrderUpdate events
- 🔄 Implement repository layer for ScyllaDB operations
- 🔄 Add E2E tests
