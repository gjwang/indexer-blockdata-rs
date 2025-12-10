-- Active Orders Table
-- Stores currently open orders for fast queries by user+symbol

CREATE TABLE IF NOT EXISTS active_orders (
    -- Partition key: (user_id, symbol_id) for O(1) queries
    user_id bigint,
    symbol_id int,

    -- Clustering columns: time-ordered
    created_at bigint,
    order_id bigint,

    -- Order details
    side tinyint,          -- 0=Buy, 1=Sell
    order_type tinyint,    -- 0=Market, 1=Limit
    price bigint,          -- Price in satoshis
    quantity bigint,       -- Total order quantity
    filled_qty bigint,     -- Filled quantity (updated incrementally)

    -- Status
    status tinyint,        -- 0=New, 1=PartialFill, 2=Filled, 3=Cancelled
    updated_at bigint,

    PRIMARY KEY ((user_id, symbol_id), created_at, order_id)
) WITH CLUSTERING ORDER BY (created_at DESC, order_id DESC)
  AND comment = 'Active orders by user+symbol for fast queries'
  AND default_time_to_live = 86400;  -- Auto-expire after 24h

-- Lookup table for partition key resolution
-- Needed because updates require full partition key
CREATE TABLE IF NOT EXISTS order_lookup (
    order_id bigint PRIMARY KEY,
    user_id bigint,
    symbol_id int,
    created_at bigint
) WITH comment = 'Partition key lookup for order updates';
