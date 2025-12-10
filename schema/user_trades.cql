-- Optimized Trade Storage: user_trades table
-- Partition key: (user_id, symbol_id) for O(1) queries
-- Clustering: settled_at DESC for time-ordered results

CREATE TABLE IF NOT EXISTS user_trades (
    -- Partition key
    user_id bigint,
    symbol_id int,

    -- Clustering columns (time DESC for recent-first)
    settled_at bigint,
    trade_id bigint,

    -- Trade metadata
    role tinyint,                    -- 0=buyer, 1=seller
    order_id bigint,                 -- User's order ID
    counterparty_user_id bigint,     -- Other party's user ID
    counterparty_order_id bigint,    -- Other party's order ID

    -- Trade details
    price bigint,                    -- Price in satoshis
    quantity bigint,                 -- Quantity in satoshis

    -- Asset information
    base_asset_id int,
    quote_asset_id int,

    -- Settlement metadata
    output_sequence bigint,
    match_seq bigint,

    PRIMARY KEY ((user_id, symbol_id), settled_at, trade_id)
) WITH CLUSTERING ORDER BY (settled_at DESC, trade_id DESC)
  AND comment = 'User trades denormalized by user+symbol for O(1) queries'
  AND gc_grace_seconds = 86400
  AND compaction = {
    'class': 'TimeWindowCompactionStrategy',
    'compaction_window_size': '1',
    'compaction_window_unit': 'DAYS'
  };

-- Optional: Add TTL for data retention (e.g., 90 days)
-- ALTER TABLE user_trades WITH default_time_to_live = 7776000;
