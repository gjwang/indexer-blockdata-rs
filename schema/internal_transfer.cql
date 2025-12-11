-- Internal Transfer Requests Table
-- 用于记录所有内部划转请求

CREATE KEYSPACE IF NOT EXISTS settlement
WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};

USE settlement;

CREATE TABLE IF NOT EXISTS balance_transfer_requests (
    request_id bigint PRIMARY KEY,

    -- 源账户
    from_account_type text,
    from_user_id bigint,
    from_asset_id int,

    -- 目标账户
    to_account_type text,
    to_user_id bigint,
    to_asset_id int,

    -- 金额 (i64 scaled)
    amount bigint,

    -- 状态
    status text,

    -- 时间戳
    created_at bigint,
    updated_at bigint,

    -- TigerBeetle transfer IDs
    pending_transfer_id bigint,
    posted_transfer_id bigint,

    -- 处理器和错误信息
    processor text,
    error_message text,

    -- 操作员 (future extension)
    operator_id bigint,
    operator_role text
);

-- Index for status queries
CREATE INDEX IF NOT EXISTS idx_transfer_status
ON balance_transfer_requests (status);

-- Index for user queries (spot transfers)
CREATE INDEX IF NOT EXISTS idx_transfer_user
ON balance_transfer_requests (from_user_id);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_transfer_created
ON balance_transfer_requests (created_at);
