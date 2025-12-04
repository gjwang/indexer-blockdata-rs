use anyhow::{Context, Result};
use scylla::{Session, SessionBuilder};
use scylla::prepared_statement::PreparedStatement;
use std::sync::Arc;
use chrono::Utc;
use std::time::Duration;
use tokio::time::sleep;

use crate::configure::ScyllaDbConfig;
use crate::ledger::MatchExecData;

// Retry configuration
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 50;

// CQL Statements
const INSERT_TRADE_CQL: &str = "
    INSERT INTO settled_trades (
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset, quote_asset,
        buyer_refund, seller_refund, settled_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
";

const SELECT_TRADE_BY_ID_CQL: &str = "
    SELECT 
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset, quote_asset,
        buyer_refund, seller_refund, settled_at
    FROM settled_trades 
    WHERE trade_id = ?
";

const SELECT_TRADES_BY_SEQ_CQL: &str = "
    SELECT 
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset, quote_asset,
        buyer_refund, seller_refund, settled_at
    FROM settled_trades_by_sequence 
    WHERE output_sequence >= ? AND output_sequence <= ?
    ALLOW FILTERING
";

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
            .prepare(INSERT_TRADE_CQL)
            .await
            .context("Failed to prepare insert statement")?;

        Ok(Self {
            session,
            keyspace: config.keyspace.clone(),
            insert_trade_stmt,
        })
    }

    /// Insert a single trade into the database with retry logic
    /// 
    /// # Arguments
    /// * `trade` - Trade data from matching engine
    /// 
    /// # Returns
    /// * `Result<()>` - Success or error
    pub async fn insert_trade(&self, trade: &MatchExecData) -> Result<()> {
        // Get current date as days since Unix epoch (for partitioning)
        let now_ts = Utc::now().timestamp();
        let trade_date = (now_ts / 86400) as i32;  // Days since Unix epoch
        let settled_at = Utc::now().timestamp_millis();

        let mut attempt = 0;
        let mut delay = INITIAL_RETRY_DELAY_MS;

        loop {
            attempt += 1;
            
            // Execute prepared statement
            let result = self.session
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
                .await;

            match result {
                Ok(_) => return Ok(()),
                Err(e) => {
                    if attempt > MAX_RETRIES {
                        return Err(e).context(format!("Failed to insert trade after {} attempts", MAX_RETRIES));
                    }
                    
                    // Log warning (using println/eprintln as we don't have logger here, or just rely on caller)
                    // Better to just sleep and retry.
                    sleep(Duration::from_millis(delay)).await;
                    delay *= 2; // Exponential backoff
                }
            }
        }
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
            let mut attempt = 0;
            let mut delay = INITIAL_RETRY_DELAY_MS;

            loop {
                attempt += 1;

                let result = self.session
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
                    .await;

                match result {
                    Ok(_) => break, // Success, move to next trade
                    Err(e) => {
                        if attempt > MAX_RETRIES {
                            return Err(e).context(format!("Failed to insert trade in batch after {} attempts", MAX_RETRIES));
                        }
                        sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                    }
                }
            }
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
    pub async fn get_trade_by_id(&self, trade_id: u64) -> Result<Option<MatchExecData>> {
        let result = self.session
            .query(SELECT_TRADE_BY_ID_CQL, (trade_id as i64,))
            .await
            .context("Failed to query trade by ID")?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(Self::parse_trade_row(row)?));
            }
        }

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
    pub async fn get_trades_by_sequence_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<Vec<MatchExecData>> {
        let result = self.session
            .query(SELECT_TRADES_BY_SEQ_CQL, (start as i64, end as i64))
            .await
            .context("Failed to query trades by sequence range")?;

        let mut trades = Vec::new();

        if let Some(rows) = result.rows {
            for row in rows {
                trades.push(Self::parse_trade_row(row)?);
            }
        }

        Ok(trades)
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

    /// Helper to parse a ScyllaDB row into MatchExecData
    fn parse_trade_row(row: scylla::frame::response::result::Row) -> Result<MatchExecData> {
        let (
            _trade_date,
            output_sequence,
            trade_id,
            match_seq,
            buy_order_id,
            sell_order_id,
            buyer_user_id,
            seller_user_id,
            price,
            quantity,
            base_asset,
            quote_asset,
            buyer_refund,
            seller_refund,
            _settled_at,
        ): (
            i32, i64, i64, i64, i64, i64, i64, i64,
            i64, i64, i32, i32, i64, i64, i64
        ) = row.into_typed().context("Failed to parse row")?;

        Ok(MatchExecData {
            trade_id: trade_id as u64,
            buy_order_id: buy_order_id as u64,
            sell_order_id: sell_order_id as u64,
            buyer_user_id: buyer_user_id as u64,
            seller_user_id: seller_user_id as u64,
            price: price as u64,
            quantity: quantity as u64,
            base_asset: base_asset as u32,
            quote_asset: quote_asset as u32,
            buyer_refund: buyer_refund as u64,
            seller_refund: seller_refund as u64,
            match_seq: match_seq as u64,
            output_sequence: output_sequence as u64,
        })
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
