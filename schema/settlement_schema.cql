-- Settlement Database Schema for ScyllaDB
-- This schema stores all settled trades from the matching engine

-- Create keyspace with SimpleStrategy (for development)
-- In production, use NetworkTopologyStrategy with appropriate replication
CREATE KEYSPACE IF NOT EXISTS settlement
WITH replication = {
    'class': 'SimpleStrategy',
    'replication_factor': 1
};

-- Use the settlement keyspace
USE settlement;

-- Main settled trades table
-- Partition key: trade_date (for time-based partitioning)
-- Clustering key: output_sequence (for ordering within partition)
CREATE TABLE IF NOT EXISTS settled_trades (
    -- Partition key: Date-based partitioning for efficient queries
    trade_date date,
    
    -- Clustering key: Ensures ordering by sequence within each day
    output_sequence bigint,
    
    -- Trade identification
    trade_id bigint,
    match_seq bigint,
    
    -- Order references
    buy_order_id bigint,
    sell_order_id bigint,
    
    -- Parties involved
    buyer_user_id bigint,
    seller_user_id bigint,
    
    -- Trade details
    price bigint,
    quantity bigint,
    base_asset int,
    quote_asset int,
    
    -- Refunds (if any)
    buyer_refund bigint,
    seller_refund bigint,
    
    -- Timestamp
    settled_at timestamp,
    
    PRIMARY KEY ((trade_date), output_sequence)
) WITH CLUSTERING ORDER BY (output_sequence ASC)
  AND compaction = {
      'class': 'TimeWindowCompactionStrategy',
      'compaction_window_size': 1,
      'compaction_window_unit': 'DAYS'
  }
  AND gc_grace_seconds = 86400
  AND comment = 'Stores all settled trades from the matching engine';

-- Secondary index on trade_id for lookups by trade ID
CREATE INDEX IF NOT EXISTS idx_trade_id ON settled_trades (trade_id);

-- Secondary index on buyer_user_id for user trade history
CREATE INDEX IF NOT EXISTS idx_buyer_user_id ON settled_trades (buyer_user_id);

-- Secondary index on seller_user_id for user trade history
CREATE INDEX IF NOT EXISTS idx_seller_user_id ON settled_trades (seller_user_id);

-- Materialized view for sequence-based queries (global ordering)
-- This allows querying trades by sequence across all partitions
CREATE MATERIALIZED VIEW IF NOT EXISTS settled_trades_by_sequence AS
    SELECT *
    FROM settled_trades
    WHERE output_sequence IS NOT NULL
      AND trade_date IS NOT NULL
    PRIMARY KEY (output_sequence, trade_date)
    WITH CLUSTERING ORDER BY (trade_date DESC);

-- Table for tracking settlement service state (for recovery)
CREATE TABLE IF NOT EXISTS settlement_state (
    service_id text,
    last_processed_sequence bigint,
    last_update_time timestamp,
    PRIMARY KEY (service_id)
) WITH comment = 'Tracks the last processed sequence for settlement service recovery';
