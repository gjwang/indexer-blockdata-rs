
-- Keyspace: trading
-- Table: internal_transfers

CREATE TABLE IF NOT EXISTS trading.internal_transfers (
    request_id bigint PRIMARY KEY,
    user_id bigint,
    from_account_type text,
    from_user_id bigint,
    to_account_type text,
    to_user_id bigint,
    asset_id int,
    amount bigint,
    status text,
    created_at bigint,
    updated_at bigint,
    error_message text
);

-- Index for querying by user_id
CREATE INDEX IF NOT EXISTS internal_transfers_user_idx ON trading.internal_transfers (user_id);
