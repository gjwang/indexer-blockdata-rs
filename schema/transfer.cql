-- Internal Transfer Table
-- Version: 2.0
-- Date: 2025-12-12

CREATE TABLE IF NOT EXISTS trading.transfers (
    req_id uuid,
    source text,           -- 'funding' or 'trading'
    target text,           -- 'funding' or 'trading'
    user_id bigint,
    asset_id int,
    amount bigint,
    state text,            -- FSM state: init, source_pending, source_done, target_pending, compensating, committed, rolled_back, failed
    created_at bigint,
    updated_at bigint,
    error text,
    retry_count int,
    PRIMARY KEY (req_id)
);

-- Note: For scanning stale transfers, we use ALLOW FILTERING
-- In production with high volume, consider:
-- 1. Secondary index on (state, updated_at)
-- 2. Materialized view for pending transfers
