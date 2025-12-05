-- Order History Service Schema
-- Active orders + Order history for complete lifecycle tracking

-- ============================================================================
-- Active Orders Table (Open Orders)
-- ============================================================================
-- Partition by user_id for fast user-specific queries
-- Orders are removed when filled or cancelled

CREATE TABLE IF NOT EXISTS active_orders (
    user_id bigint,
    order_id bigint,
    client_order_id text,
    symbol text,
    side text,              -- 'Buy' or 'Sell'
    order_type text,        -- 'Limit' or 'Market'
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    status text,            -- 'New', 'PartiallyFilled'
    created_at bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, order_id)
) WITH CLUSTERING ORDER BY (order_id DESC);

-- Index for querying by symbol
CREATE INDEX IF NOT EXISTS idx_active_orders_symbol ON active_orders (symbol);

-- ============================================================================
-- Order History Table (Complete Order Lifecycle)
-- ============================================================================
-- Partition by user_id for fast user-specific queries
-- Cluster by created_at DESC for chronological ordering
-- Stores ALL order events: New, PartiallyFilled, Filled, Cancelled, Rejected

CREATE TABLE IF NOT EXISTS order_history (
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
    status text,            -- 'New', 'PartiallyFilled', 'Filled', 'Cancelled', 'Rejected', 'Expired'
    rejection_reason text,
    match_id bigint,        -- For tracking which match filled the order
    updated_at bigint,
    PRIMARY KEY (user_id, created_at, order_id)
) WITH CLUSTERING ORDER BY (created_at DESC, order_id DESC);

-- Index for querying by symbol
CREATE INDEX IF NOT EXISTS idx_order_history_symbol ON order_history (symbol);

-- Index for querying by status
CREATE INDEX IF NOT EXISTS idx_order_history_status ON order_history (status);

-- Index for querying by order_id (for direct order lookup)
CREATE INDEX IF NOT EXISTS idx_order_history_order_id ON order_history (order_id);

-- ============================================================================
-- Order Updates Stream Table (Event Sourcing)
-- ============================================================================
-- Stores every OrderUpdate event for audit trail and replay
-- Partition by date for efficient time-based queries

CREATE TABLE IF NOT EXISTS order_updates_stream (
    event_date int,         -- YYYYMMDD format for partitioning
    event_id bigint,        -- Monotonic sequence
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
) WITH CLUSTERING ORDER BY (event_id DESC);

-- Index for querying by order_id
CREATE INDEX IF NOT EXISTS idx_updates_order_id ON order_updates_stream (order_id);

-- Index for querying by user_id
CREATE INDEX IF NOT EXISTS idx_updates_user_id ON order_updates_stream (user_id);

-- ============================================================================
-- Order Statistics Table (Aggregated Metrics)
-- ============================================================================
-- Per-user order statistics for analytics

CREATE TABLE IF NOT EXISTS order_statistics (
    user_id bigint PRIMARY KEY,
    total_orders bigint,
    filled_orders bigint,
    cancelled_orders bigint,
    rejected_orders bigint,
    total_volume bigint,        -- Total traded volume
    last_order_at bigint,
    updated_at bigint
);

-- ============================================================================
-- Notes:
-- ============================================================================
-- 1. active_orders: Fast lookup for open orders (user_id partition)
-- 2. order_history: Complete audit trail (user_id partition, time-ordered)
-- 3. order_updates_stream: Event sourcing for replay (date partition)
-- 4. order_statistics: Aggregated metrics for analytics
--
-- Query Patterns:
-- - Get user's active orders: SELECT * FROM active_orders WHERE user_id = ?
-- - Get user's order history: SELECT * FROM order_history WHERE user_id = ? LIMIT 100
-- - Get specific order: SELECT * FROM order_history WHERE order_id = ? (via index)
-- - Replay events for date: SELECT * FROM order_updates_stream WHERE event_date = ?
