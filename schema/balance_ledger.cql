-- Append-Only Balance Ledger Schema
-- Each row is immutable - insert only, never update
-- Full audit trail of all balance changes

USE trading;

-- Balance Ledger: Append-only log of all balance changes
CREATE TABLE IF NOT EXISTS balance_ledger (
    user_id bigint,
    asset_id int,
    seq bigint,                    -- Sequence number (version), monotonically increasing

    -- Delta (change amount)
    delta_avail bigint,            -- Change to available: +deposit, -lock, +unlock, -withdraw, +trade_gain, -trade_spend
    delta_frozen bigint,           -- Change to frozen: +lock, -unlock, -trade_spend

    -- Running balance AFTER this event
    balance_avail bigint,          -- Available balance after this event
    balance_frozen bigint,         -- Frozen balance after this event

    -- Event metadata
    event_type text,               -- 'deposit', 'withdraw', 'lock', 'unlock', 'trade_debit', 'trade_credit', 'refund'
    ref_id bigint,                 -- Reference ID (trade_id, order_id, transfer_id, etc.)
    created_at bigint,             -- Unix timestamp in milliseconds

    PRIMARY KEY ((user_id, asset_id), seq)
) WITH CLUSTERING ORDER BY (seq DESC);  -- Latest events first

-- Index for querying by event type
CREATE INDEX IF NOT EXISTS idx_balance_ledger_event ON balance_ledger (event_type);

-- Index for querying by reference ID (find all events for a trade)
CREATE INDEX IF NOT EXISTS idx_balance_ledger_ref ON balance_ledger (ref_id);

-- Materialized View: Latest balance per user/asset (curr balance = highest seq row)
-- This gives O(1) lookup for current balance
-- Note: With clustering order DESC, LIMIT 1 returns highest seq
CREATE MATERIALIZED VIEW IF NOT EXISTS balance_current AS
    SELECT user_id, asset_id, seq, balance_avail, balance_frozen, created_at
    FROM balance_ledger
    WHERE user_id IS NOT NULL
      AND asset_id IS NOT NULL
      AND seq IS NOT NULL
    PRIMARY KEY ((user_id, asset_id), seq)
    WITH CLUSTERING ORDER BY (seq DESC);

-- Note: Current balance = SELECT * FROM balance_current WHERE user_id = ? AND asset_id = ? LIMIT 1
