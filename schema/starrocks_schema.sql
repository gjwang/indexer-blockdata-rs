CREATE DATABASE IF NOT EXISTS settlement;

USE settlement;

-- Trades table (OLAP optimized)
CREATE TABLE IF NOT EXISTS trades (
    trade_date DATE NOT NULL,
    trade_id BIGINT NOT NULL,
    match_seq BIGINT NOT NULL,
    base_asset INT NOT NULL,
    quote_asset INT NOT NULL,
    price DECIMAL(18, 8) NOT NULL,
    quantity DECIMAL(18, 8) NOT NULL,
    buyer_user_id BIGINT NOT NULL,
    seller_user_id BIGINT NOT NULL,
    settled_at DATETIME NOT NULL
)
ENGINE=OLAP
DUPLICATE KEY(trade_date, trade_id)
PARTITION BY RANGE(trade_date) (
    START ("2024-01-01") END ("2026-01-01") EVERY (INTERVAL 1 MONTH)
)
DISTRIBUTED BY HASH(trade_id) BUCKETS 10
PROPERTIES (
    "replication_num" = "1"
);

-- Candles table (Aggregated)
CREATE TABLE IF NOT EXISTS candles (
    base_asset INT NOT NULL,
    quote_asset INT NOT NULL,
    interval_str VARCHAR(10) NOT NULL, -- '1m', '1h', '1d'
    event_time DATETIME NOT NULL,
    open DECIMAL(18, 8) NOT NULL,
    high DECIMAL(18, 8) NOT NULL,
    low DECIMAL(18, 8) NOT NULL,
    close DECIMAL(18, 8) NOT NULL,
    volume DECIMAL(18, 8) NOT NULL
)
ENGINE=OLAP
PRIMARY KEY(base_asset, quote_asset, interval_str, event_time)
PARTITION BY RANGE(event_time) (
    START ("2024-01-01") END ("2026-01-01") EVERY (INTERVAL 1 MONTH)
)
DISTRIBUTED BY HASH(base_asset, quote_asset) BUCKETS 10
PROPERTIES (
    "replication_num" = "1"
);
