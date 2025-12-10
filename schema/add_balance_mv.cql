-- Materialized View for efficient user balance queries
-- Automatically maintained by ScyllaDB
-- This enables O(1) lookups by user_id (returns all assets for a user)

CREATE MATERIALIZED VIEW IF NOT EXISTS trading.user_balances_by_user AS
    SELECT user_id, asset_id, seq, avail, frozen, created_at
    FROM trading.balance_ledger
    WHERE user_id IS NOT NULL
      AND asset_id IS NOT NULL
      AND seq IS NOT NULL
    PRIMARY KEY (user_id, asset_id, seq)
    WITH CLUSTERING ORDER BY (asset_id ASC, seq DESC)
    AND compaction = {
        'class': 'SizeTieredCompactionStrategy',
        'min_threshold': 4,
        'max_threshold': 32
    }
    AND compression = {
        'sstable_compression': 'LZ4Compressor'
    }
    AND bloom_filter_fp_chance = 0.01
    AND caching = {
        'keys': 'ALL',
        'rows_per_partition': 'ALL'
    };

-- Query usage:
-- SELECT * FROM user_balances_by_user
-- WHERE user_id = ?
-- PER PARTITION LIMIT 1;
--
-- This returns latest balance per asset for the user
