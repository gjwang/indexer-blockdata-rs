-- Unified Settlement Schema (Matches Rust Code)
CREATE KEYSPACE IF NOT EXISTS trading WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};

USE trading;

-- Generic Settled Trades Table
CREATE TABLE IF NOT EXISTS settled_trades (
    trade_id bigint,
    output_sequence bigint,
    match_seq bigint,
    buy_order_id bigint,
    sell_order_id bigint,
    buyer_user_id bigint,
    seller_user_id bigint,
    price bigint,
    quantity bigint,
    base_asset int,
    quote_asset int,
    buyer_refund bigint,
    seller_refund bigint,
    settled_at bigint,
    trade_date int,
    buyer_base_version bigint,
    buyer_quote_version bigint,
    seller_base_version bigint,
    seller_quote_version bigint,
    buyer_base_balance_after bigint,
    buyer_quote_balance_after bigint,
    seller_base_balance_after bigint,
    seller_quote_balance_after bigint,
    PRIMARY KEY (trade_id)
);

CREATE INDEX IF NOT EXISTS idx_settled_trades_buyer ON settled_trades (buyer_user_id);
CREATE INDEX IF NOT EXISTS idx_settled_trades_seller ON settled_trades (seller_user_id);
CREATE INDEX IF NOT EXISTS idx_settled_trades_sequence ON settled_trades (output_sequence);
CREATE INDEX IF NOT EXISTS idx_settled_trades_date ON settled_trades (trade_date);

-- Materialized View for Sequence lookup
CREATE MATERIALIZED VIEW IF NOT EXISTS settled_trades_by_sequence AS
    SELECT *
    FROM settled_trades
    WHERE output_sequence IS NOT NULL AND trade_id IS NOT NULL
    PRIMARY KEY (output_sequence, trade_id);

-- User Balances Table
CREATE TABLE IF NOT EXISTS user_balances (
    user_id bigint,
    asset_id int,
    avail bigint,
    frozen bigint,
    version bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, asset_id)
);

-- Ledger Events
CREATE TABLE IF NOT EXISTS ledger_events (
    user_id bigint,
    sequence_id bigint,
    event_type text,
    amount bigint,
    currency int,
    related_id bigint,
    created_at bigint,
    PRIMARY KEY (user_id, sequence_id)
);
CREATE INDEX IF NOT EXISTS idx_ledger_events_seq ON ledger_events (sequence_id);

-- Active Orders
CREATE TABLE IF NOT EXISTS active_orders (
    user_id bigint,
    order_id bigint,
    client_order_id text,
    symbol_id int,
    side tinyint,
    order_type tinyint,
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    status tinyint,
    created_at bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, order_id)
);

-- Order History
CREATE TABLE IF NOT EXISTS order_history (
    user_id bigint,
    order_id bigint,
    client_order_id text,
    symbol_id int,
    side tinyint,
    order_type tinyint,
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    match_id bigint,
    status tinyint,
    rejection_reason text,
    created_at bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, order_id)
);

-- Order Updates Stream
CREATE TABLE IF NOT EXISTS order_updates_stream (
    event_id bigint,
    event_date int,
    order_id bigint,
    user_id bigint,
    client_order_id text,
    symbol_id int,
    status tinyint,
    price bigint,
    qty bigint,
    filled_qty bigint,
    avg_fill_price bigint,
    match_id bigint,
    rejection_reason text,
    timestamp bigint,
    PRIMARY KEY (event_id)
);

-- Order Statistics (Using simple table with RMW logic in app)
CREATE TABLE IF NOT EXISTS order_statistics (
    user_id bigint,
    total_orders int,
    filled_orders int,
    cancelled_orders int,
    rejected_orders int,
    total_volume bigint,
    last_order_at bigint,
    updated_at bigint,
    PRIMARY KEY (user_id)
);
