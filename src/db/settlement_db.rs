use anyhow::{Context, Result};
use scylla::{Session, SessionBuilder};
use scylla::prepared_statement::PreparedStatement;
use std::sync::Arc;
use chrono::Utc;

use crate::configure::ScyllaDbConfig;
use crate::ledger::MatchExecData;

/// Settlement database client for ScyllaDB
/// 
/// Provides a clean abstraction for storing and querying settled trades.
pub struct SettlementDb {
    session: Arc<Session>,
    keyspace: String,
    
    // Prepared statements for performance
    insert_trade_stmt: PreparedStatement,
}

impl SettlementDb {
    /// Connect to ScyllaDB and prepare statements
    /// 
    /// # Arguments
    /// * `config` - ScyllaDB configuration
    /// 
    /// # Returns
    /// * `Result<SettlementDb>` - Connected database client
    pub async fn connect(config: &ScyllaDbConfig) -> Result<Self> {
        // Build session
        let session: Session = SessionBuilder::new()
            .known_nodes(&config.hosts)
            .connection_timeout(std::time::Duration::from_millis(config.connection_timeout_ms))
            .build()
            .await
            .context("Failed to connect to ScyllaDB")?;

        let session = Arc::new(session);
        
        // Use the settlement keyspace
        session
            .query(format!("USE {}", config.keyspace), &[])
            .await
            .context("Failed to use settlement keyspace")?;

        // Prepare insert statement
        let insert_trade_stmt = session
            .prepare(
                "INSERT INTO settled_trades (
                    trade_date, output_sequence, trade_id, match_seq,
                    buy_order_id, sell_order_id,
                    buyer_user_id, seller_user_id,
                    price, quantity, base_asset, quote_asset,
                    buyer_refund, seller_refund, settled_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .await
            .context("Failed to prepare insert statement")?;

        Ok(Self {
            session,
            keyspace: config.keyspace.clone(),
            insert_trade_stmt,
        })
    }

    /// Insert a single trade into the database
    /// 
    /// # Arguments
    /// * `trade` - Trade data from matching engine
    /// 
    /// # Returns
    /// * `Result<()>` - Success or error
    pub async fn insert_trade(&self, trade: &MatchExecData) -> Result<()> {
        // Get current date as days since Unix epoch (for partitioning)
        // Unix epoch is 1970-01-01, so divide timestamp by seconds per day
        let now_ts = Utc::now().timestamp();
        let trade_date = (now_ts / 86400) as i32;  // Days since Unix epoch
        let settled_at = Utc::now().timestamp_millis();

        // Execute prepared statement
        self.session
            .execute(
                &self.insert_trade_stmt,
                (
                    trade_date,
                    trade.output_sequence as i64,
                    trade.trade_id as i64,
                    trade.match_seq as i64,
                    trade.buy_order_id as i64,
                    trade.sell_order_id as i64,
                    trade.buyer_user_id as i64,
                    trade.seller_user_id as i64,
                    trade.price as i64,
                    trade.quantity as i64,
                    trade.base_asset as i32,
                    trade.quote_asset as i32,
                    trade.buyer_refund as i64,
                    trade.seller_refund as i64,
                    settled_at,
                ),
            )
            .await
            .context("Failed to insert trade")?;

        Ok(())
    }

    /// Insert multiple trades in a batch (more efficient)
    /// 
    /// # Arguments
    /// * `trades` - Slice of trades to insert
    /// 
    /// # Returns
    /// * `Result<()>` - Success or error
    pub async fn insert_batch(&self, trades: &[MatchExecData]) -> Result<()> {
        if trades.is_empty() {
            return Ok(());
        }

        let now_ts = Utc::now().timestamp();
        let trade_date = (now_ts / 86400) as i32;
        let settled_at = Utc::now().timestamp_millis();

        // Execute all inserts (ScyllaDB will batch them automatically)
        for trade in trades {
            self.session
                .execute(
                    &self.insert_trade_stmt,
                    (
                        trade_date,
                        trade.output_sequence as i64,
                        trade.trade_id as i64,
                        trade.match_seq as i64,
                        trade.buy_order_id as i64,
                        trade.sell_order_id as i64,
                        trade.buyer_user_id as i64,
                        trade.seller_user_id as i64,
                        trade.price as i64,
                        trade.quantity as i64,
                        trade.base_asset as i32,
                        trade.quote_asset as i32,
                        trade.buyer_refund as i64,
                        trade.seller_refund as i64,
                        settled_at,
                    ),
                )
                .await
                .context("Failed to insert trade in batch")?;
        }

        Ok(())
    }

    /// Get a trade by its trade ID
    /// 
    /// # Arguments
    /// * `trade_id` - Trade ID to look up
    /// 
    /// # Returns
    /// * `Result<Option<MatchExecData>>` - Trade data if found
    /// 
    /// Note: This is a simplified version that returns raw query results.
    /// Full implementation would parse all fields properly.
    pub async fn get_trade_by_id(&self, _trade_id: u64) -> Result<Option<MatchExecData>> {
        // TODO: Implement proper row parsing with ScyllaDB value types
        // For now, this is a placeholder
        Ok(None)
    }

    /// Get trades by sequence range
    /// 
    /// # Arguments
    /// * `start` - Start sequence (inclusive)
    /// * `end` - End sequence (inclusive)
    /// 
    /// # Returns
    /// * `Result<Vec<MatchExecData>>` - List of trades in range
    /// 
    /// Note: This is a simplified version.
    /// Full implementation would use the materialized view and parse results.
    pub async fn get_trades_by_sequence_range(
        &self,
        _start: u64,
        _end: u64,
    ) -> Result<Vec<MatchExecData>> {
        // TODO: Implement proper row parsing with ScyllaDB value types
        // For now, return empty vector
        Ok(Vec::new())
    }

    /// Health check - verify database connectivity
    /// 
    /// # Returns
    /// * `Result<bool>` - True if connected and healthy
    pub async fn health_check(&self) -> Result<bool> {
        self.session
            .query("SELECT now() FROM system.local", &[])
            .await
            .context("Health check failed")?;
        
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires running ScyllaDB
    async fn test_connect() {
        let config = ScyllaDbConfig {
            hosts: vec!["localhost:9042".to_string()],
            keyspace: "settlement".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 3000,
        };

        let db = SettlementDb::connect(&config).await;
        assert!(db.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires running ScyllaDB
    async fn test_health_check() {
        let config = ScyllaDbConfig {
            hosts: vec!["localhost:9042".to_string()],
            keyspace: "settlement".to_string(),
            replication_factor: 1,
            connection_timeout_ms: 5000,
            request_timeout_ms: 3000,
        };

        let db = SettlementDb::connect(&config).await.unwrap();
        let health = db.health_check().await;
        assert!(health.is_ok());
        assert!(health.unwrap());
    }
}
