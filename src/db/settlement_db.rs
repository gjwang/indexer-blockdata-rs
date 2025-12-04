use anyhow::{Context, Result};
use chrono::Utc;
use scylla::prepared_statement::PreparedStatement;
use scylla::{Session, SessionBuilder};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::configure::ScyllaDbConfig;
use crate::ledger::{MatchExecData, LedgerEvent};

// Retry configuration
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 50;
const SLOW_QUERY_THRESHOLD_MS: u128 = 100;

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

const INSERT_LEDGER_EVENT_CQL: &str = "
    INSERT INTO ledger_events (
        user_id, sequence_id, event_type, amount, currency, related_id, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?)
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
    insert_ledger_event_stmt: PreparedStatement,
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
            .context("Failed to prepare insert trade statement")?;

        let insert_ledger_event_stmt = session
            .prepare(INSERT_LEDGER_EVENT_CQL)
            .await
            .context("Failed to prepare insert ledger event statement")?;

        Ok(Self {
            session,
            keyspace: config.keyspace.clone(),
            insert_trade_stmt,
            insert_ledger_event_stmt,
        })
    }

    /// Execute an async operation with exponential backoff retry
    async fn retry_with_backoff<F, Fut, T, E>(operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let mut attempt = 0;
        let mut delay = INITIAL_RETRY_DELAY_MS;

        loop {
            attempt += 1;
            match operation().await {
                Ok(val) => return Ok(val),
                Err(e) => {
                    if attempt > MAX_RETRIES {
                        return Err(anyhow::Error::new(e))
                            .context(format!("Operation failed after {} attempts", MAX_RETRIES));
                    }
                    sleep(Duration::from_millis(delay)).await;
                    delay *= 2;
                }
            }
        }
    }

    /// Insert a single trade into the database with retry logic
    ///
    /// # Arguments
    /// * `trade` - Trade data from matching engine
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub async fn insert_trade(&self, trade: &MatchExecData) -> Result<()> {
        let start = std::time::Instant::now();

        let trade_date = crate::common_utils::get_current_date();
        let settled_at = Utc::now().timestamp_millis();

        let result = Self::retry_with_backoff(|| async {
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
                .map(|_| ())
        })
        .await;

        let duration = start.elapsed();
        let duration_ms = duration.as_millis();

        if duration_ms > SLOW_QUERY_THRESHOLD_MS {
            log::warn!("Slow insert_trade: {}ms", duration_ms);
        }

        // Log metric (debug level to avoid spamming info)
        log::debug!("[METRIC] settlement_db_insert_latency_ms={}", duration_ms);

        result
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

        let start = std::time::Instant::now();

        let trade_date = crate::common_utils::get_current_date();
        let settled_at = Utc::now().timestamp_millis();

        // Execute all inserts (ScyllaDB will batch them automatically)
        for trade in trades {
            Self::retry_with_backoff(|| async {
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
                    .map(|_| ())
            })
            .await?;
        }

        let duration = start.elapsed();
        let duration_ms = duration.as_millis();

        if duration_ms > SLOW_QUERY_THRESHOLD_MS {
            log::warn!("Slow insert_batch ({} trades): {}ms", trades.len(), duration_ms);
        }

        log::debug!("[METRIC] settlement_db_batch_insert_latency_ms={}", duration_ms);

        Ok(())
    }

    /// Insert a ledger event into the database
    pub async fn insert_ledger_event(&self, event: &LedgerEvent) -> Result<()> {
        let start = std::time::Instant::now();

        let result = Self::retry_with_backoff(|| async {
            self.session
                .execute(
                    &self.insert_ledger_event_stmt,
                    (
                        event.user_id as i64,
                        event.sequence_id as i64,
                        &event.event_type,
                        event.amount as i64,
                        event.currency as i32,
                        event.related_id as i64,
                        event.created_at,
                    ),
                )
                .await
                .map(|_| ())
        }).await;

        let duration = start.elapsed();
        let duration_ms = duration.as_millis();

        if duration_ms > SLOW_QUERY_THRESHOLD_MS {
            log::warn!("Slow insert_ledger_event: {}ms", duration_ms);
        }

        log::debug!("[METRIC] settlement_db_event_insert_latency_ms={}", duration_ms);

        result
    }

    /// Get a trade by its trade ID
    ///
    /// # Arguments
    /// * `trade_id` - Trade ID to look up
    ///
    /// # Returns
    /// * `Result<Option<MatchExecData>>` - Trade data if found
    pub async fn get_trade_by_id(&self, trade_id: u64) -> Result<Option<MatchExecData>> {
        let result = self
            .session
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
        let result = self
            .session
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
        ): (i32, i64, i64, i64, i64, i64, i64, i64, i64, i64, i32, i32, i64, i64, i64) =
            row.into_typed().context("Failed to parse row")?;

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

    #[tokio::test]
    async fn test_retry_logic() {
        use std::fmt;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        #[derive(Debug)]
        struct MockError;
        impl fmt::Display for MockError {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "Mock Error")
            }
        }
        impl std::error::Error for MockError {}

        // Test case 1: Succeeds immediately
        let result = SettlementDb::retry_with_backoff(|| async { Ok::<_, MockError>(()) }).await;
        assert!(result.is_ok());

        // Test case 2: Fails twice then succeeds
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = SettlementDb::retry_with_backoff(|| async {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(MockError)
            } else {
                Ok(())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3); // 0 (fail), 1 (fail), 2 (success)

        // Test case 3: Fails more than MAX_RETRIES (3)
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = SettlementDb::retry_with_backoff(|| async {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(MockError)
        })
        .await;

        assert!(result.is_err());
        // Should try 1 initial + 3 retries = 4 attempts
        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }
}
