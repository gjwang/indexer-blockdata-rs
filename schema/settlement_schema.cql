-- Settlement Service Schema
-- Per-symbol trade tables + User balances + Reconciliation

-- ============================================================================
-- Per-Symbol Trade Tables (Source of Truth)
-- ============================================================================

-- BTC/USDT trades
CREATE TABLE IF NOT EXISTS settled_trades_btc_usdt (
    trade_id bigint PRIMARY KEY,
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
    -- Balance versions for tracking
    buyer_base_version bigint,
    buyer_quote_version bigint,
    seller_base_version bigint,
    seller_quote_version bigint
);

-- Indexes for queries
CREATE INDEX IF NOT EXISTS idx_btc_usdt_buyer ON settled_trades_btc_usdt (buyer_user_id);
CREATE INDEX IF NOT EXISTS idx_btc_usdt_seller ON settled_trades_btc_usdt (seller_user_id);
CREATE INDEX IF NOT EXISTS idx_btc_usdt_sequence ON settled_trades_btc_usdt (output_sequence);
CREATE INDEX IF NOT EXISTS idx_btc_usdt_date ON settled_trades_btc_usdt (trade_date);

-- ETH/USDT trades
CREATE TABLE IF NOT EXISTS settled_trades_eth_usdt (
    trade_id bigint PRIMARY KEY,
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
    seller_quote_version bigint
);

CREATE INDEX IF NOT EXISTS idx_eth_usdt_buyer ON settled_trades_eth_usdt (buyer_user_id);
CREATE INDEX IF NOT EXISTS idx_eth_usdt_seller ON settled_trades_eth_usdt (seller_user_id);
CREATE INDEX IF NOT EXISTS idx_eth_usdt_sequence ON settled_trades_eth_usdt (output_sequence);
CREATE INDEX IF NOT EXISTS idx_eth_usdt_date ON settled_trades_eth_usdt (trade_date);

-- ============================================================================
-- User Balances Table (Materialized View for Fast Queries)
-- ============================================================================

CREATE TABLE IF NOT EXISTS user_balances (
    user_id bigint,
    asset_id int,
    available bigint,
    frozen bigint,
    version bigint,
    updated_at bigint,
    PRIMARY KEY (user_id, asset_id)
) WITH CLUSTERING ORDER BY (asset_id ASC);

-- ============================================================================
-- Reconciliation Tables
-- ============================================================================

CREATE TABLE IF NOT EXISTS reconciliation_jobs (
    job_id uuid PRIMARY KEY,
    started_at bigint,
    completed_at bigint,
    trades_checked bigint,
    errors_found bigint,
    errors_fixed bigint,
    status text
);

CREATE TABLE IF NOT EXISTS reconciliation_errors (
    error_id uuid PRIMARY KEY,
    job_id uuid,
    trade_id bigint,
    user_id bigint,
    asset_id int,
    expected_balance bigint,
    actual_balance bigint,
    fixed boolean,
    detected_at bigint,
    fixed_at bigint
);

CREATE INDEX IF NOT EXISTS idx_recon_errors_user ON reconciliation_errors (user_id);
CREATE INDEX IF NOT EXISTS idx_recon_errors_fixed ON reconciliation_errors (fixed);

-- ============================================================================
-- Ledger Events Table (for deposits/withdrawals)
-- ============================================================================

CREATE TABLE IF NOT EXISTS ledger_events (
    event_id bigint PRIMARY KEY,
    sequence_id bigint,
    user_id bigint,
    asset_id int,
    event_type text,
    amount bigint,
    balance_after bigint,
    created_at bigint
);

CREATE INDEX IF NOT EXISTS idx_ledger_user ON ledger_events (user_id);
CREATE INDEX IF NOT EXISTS idx_ledger_sequence ON ledger_events (sequence_id);
