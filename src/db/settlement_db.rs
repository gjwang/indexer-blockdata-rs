use anyhow::{Context, Result};
use chrono::Utc;
use scylla::prepared_statement::PreparedStatement;
use scylla::{Session, SessionBuilder};
use scylla::batch::Batch;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::common_utils::{get_current_date, get_current_timestamp_ms};
use crate::configure::ScyllaDbConfig;
use crate::ledger::{LedgerEvent, MatchExecData};

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

const SELECT_LEDGER_EVENTS_BY_USER_CQL: &str = "
    SELECT user_id, sequence_id, event_type, amount, currency, related_id, created_at
    FROM ledger_events
    WHERE user_id = ?
";

const SELECT_TRADES_BY_BUYER_CQL: &str = "
    SELECT
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset, quote_asset,
        buyer_refund, seller_refund, settled_at
    FROM settled_trades
    WHERE buyer_user_id = ?
    LIMIT ?
    ALLOW FILTERING
";

const SELECT_TRADES_BY_SELLER_CQL: &str = "
    SELECT
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset, quote_asset,
        buyer_refund, seller_refund, settled_at
    FROM settled_trades
    WHERE seller_user_id = ?
    LIMIT ?
    ALLOW FILTERING
";

/// Settlement database client for ScyllaDB
///
/// Provides a clean abstraction for storing and querying settled trades.
#[derive(Clone)]
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

    /// Get reference to the ScyllaDB session for direct queries
    pub fn session(&self) -> &Session {
        &self.session
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

        let trade_date = get_current_date();
        let settled_at = get_current_timestamp_ms();

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

        let trade_date = get_current_date();
        let settled_at = get_current_timestamp_ms();

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
        })
        .await;

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

    /// Get ledger events by user ID
    pub async fn get_ledger_events_by_user(&self, user_id: u64) -> Result<Vec<LedgerEvent>> {
        let result = self
            .session
            .query(SELECT_LEDGER_EVENTS_BY_USER_CQL, (user_id as i64,))
            .await
            .context("Failed to query ledger events by user")?;

        let mut events = Vec::new();

        if let Some(rows) = result.rows {
            for row in rows {
                let (user_id, sequence_id, event_type, amount, currency, related_id, created_at): (
                    i64,
                    i64,
                    String,
                    i64,
                    i32,
                    i64,
                    i64,
                ) = row.into_typed().context("Failed to parse ledger event row")?;

                events.push(LedgerEvent {
                    user_id: user_id as u64,
                    sequence_id: sequence_id as u64,
                    event_type,
                    amount: amount as u64,
                    currency: currency as u32,
                    related_id: related_id as u64,
                    created_at,
                });
            }
        }

        Ok(events)
    }

    /// Get trades by user ID (both buyer and seller roles)
    pub async fn get_trades_by_user(&self, user_id: u64, limit: i32) -> Result<Vec<MatchExecData>> {
        // Query as buyer
        let buyer_result = self
            .session
            .query(SELECT_TRADES_BY_BUYER_CQL, (user_id as i64, limit))
            .await
            .context("Failed to query trades by buyer")?;

        // Query as seller
        let seller_result = self
            .session
            .query(SELECT_TRADES_BY_SELLER_CQL, (user_id as i64, limit))
            .await
            .context("Failed to query trades by seller")?;

        let mut trades = Vec::new();

        if let Some(rows) = buyer_result.rows {
            for row in rows {
                trades.push(Self::parse_trade_row(row)?);
            }
        }

        if let Some(rows) = seller_result.rows {
            for row in rows {
                trades.push(Self::parse_trade_row(row)?);
            }
        }

        // Sort by output_sequence descending (or settled_at)
        trades.sort_by(|a, b| b.output_sequence.cmp(&a.output_sequence));

        // Truncate to limit
        if trades.len() > limit as usize {
            trades.truncate(limit as usize);
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
            settled_at,
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
            settled_at: settled_at as u64,
            // Note: Old data doesn't have version fields, default to 0
            buyer_quote_version: 0,
            buyer_base_version: 0,
            seller_base_version: 0,
            seller_quote_version: 0,
        })
    }

    /// Update user balance with version-based idempotency
    ///
    /// Uses Lightweight Transactions (LWT) to ensure idempotent updates.
    /// Only updates if the current version matches the expected version.
    ///
    /// # Arguments
    /// * `user_id` - User ID
    /// * `asset_id` - Asset ID
    /// * `delta` - Balance change (positive = increase, negative = decrease)
    /// * `expected_version` - Expected current version from matching engine
    ///
    /// # Returns
    /// * `Ok(true)` - Update successful
    /// * `Ok(false)` - Version conflict (already updated)
    /// * `Err(_)` - Database error
    pub async fn update_balance(
        &self,
        user_id: u64,
        asset_id: u32,
        delta: i64,
        _frozen_delta: i64,
        _version: u64,
    ) -> Result<()> {
        for _ in 0..10 {
            let current = self.get_user_balance(user_id, asset_id).await?;
            let (current_avail, current_ver) = match current {
                Some(b) => (b.avail as i64, b.version),
                None => {
                    anyhow::bail!("Balance not found for user {} asset {}", user_id, asset_id);
                }
            };

            let new_avail = current_avail + delta;
            if new_avail < 0 {
                anyhow::bail!("Insufficient balance");
            }

            let new_ver = current_ver + 1;
            let now = get_current_timestamp_ms();

            let query = "
                UPDATE user_balances
                SET avail = ?, version = ?, updated_at = ?
                WHERE user_id = ? AND asset_id = ?
                IF version = ?
            ";

            let result = self
                .session
                .query(
                    query,
                    (
                        new_avail,
                        new_ver as i64,
                        now,
                        user_id as i64,
                        asset_id as i32,
                        current_ver as i64,
                    ),
                )
                .await?;

            if let Some(rows) = result.rows {
                if let Some(row) = rows.into_iter().next() {
                    if let Ok((applied,)) = row.into_typed::<(bool,)>() {
                        if applied {
                            return Ok(());
                        }
                    }
                }
            }
        }
        anyhow::bail!("Failed to update balance after retries");
    }

    pub async fn update_balance_with_version(
        &self,
        user_id: u64,
        asset_id: u32,
        delta: i64,
        expected_version: u64,
    ) -> Result<bool> {
        // First, get current balance
        let current = self.get_user_balance(user_id, asset_id).await?;

        let (current_avail, current_version) = match current {
            Some(balance) => {
                //TODO: maybe bad idea to allow gaps for non-persisted events like Locks
                // Verify version matches or is older (allowing gaps for non-persisted events like Locks)
                if balance.version >= expected_version {
                    log::debug!(
                        "Balance update skipped (already processed/newer): user={}, asset={}, expected={}, actual={}",
                        user_id,
                        asset_id,
                        expected_version,
                        balance.version
                    );
                    return Ok(false);
                }
                (balance.avail as i64, balance.version)
            }
            None => {
                anyhow::bail!("Balance not found for user {} asset {}", user_id, asset_id);
            }
        };

        // Calculate new balance
        let new_avail = current_avail + delta;
        if new_avail < 0 {
            anyhow::bail!(
                "Balance would go negative: user={}, asset={}, current={}, delta={}",
                user_id,
                asset_id,
                current_avail,
                delta
            );
        }

        // Update with LWT
        // We set version to expected_version + 1 to sync with ME, skipping any gaps
        const UPDATE_BALANCE_CQL: &str = "
            UPDATE user_balances
            SET avail = ?,
                version = ?,
                updated_at = ?
            WHERE user_id = ? AND asset_id = ?
            IF version = ?
        ";

        let now = get_current_timestamp_ms();
        let new_version = expected_version + 1;

        let result = self
            .session
            .query(
                UPDATE_BALANCE_CQL,
                (
                    new_avail,
                    new_version as i64,
                    now,
                    user_id as i64,
                    asset_id as i32,
                    current_version as i64,
                ),
            )
            .await?;

        // Check LWT result
        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                // LWT returns [applied] column
                if let Ok((applied,)) = row.into_typed::<(bool,)>() {
                    if !applied {
                        log::debug!(
                            "Balance update skipped (version conflict): user={}, asset={}, expected_v={}",
                            user_id,
                            asset_id,
                            current_version
                        );
                        return Ok(false);
                    }

                    log::debug!(
                        "Balance updated: user={}, asset={}, {} -> {}, v={} -> {}",
                        user_id,
                        asset_id,
                        current_avail,
                        new_avail,
                        current_version,
                        new_version
                    );
                    return Ok(true);
                }
            }
        }

        // If we get here, something unexpected happened
        Ok(false)
    }

    /// Initialize balance record if it doesn't exist
    async fn init_balance_if_not_exists(&self, user_id: u64, asset_id: u32) -> Result<()> {
        const INIT_BALANCE_CQL: &str = "
            INSERT INTO user_balances (user_id, asset_id, avail, frozen, version, updated_at)
            VALUES (?, ?, 0, 0, 0, ?)
            IF NOT EXISTS
        ";

        let now = get_current_timestamp_ms();

        self.session.query(INIT_BALANCE_CQL, (user_id as i64, asset_id as i32, now)).await?;

        Ok(())
    }

    /// Get user balance for a specific asset
    pub async fn get_user_balance(
        &self,
        user_id: u64,
        asset_id: u32,
    ) -> Result<Option<UserBalance>> {
        const GET_BALANCE_CQL: &str = "
            SELECT avail, frozen, version, updated_at
            FROM user_balances
            WHERE user_id = ? AND asset_id = ?
        ";

        let result = self.session.query(GET_BALANCE_CQL, (user_id as i64, asset_id as i32)).await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                let (avail, frozen, version, updated_at): (i64, Option<i64>, i64, i64) =
                    row.into_typed().context("Failed to parse balance row")?;

                return Ok(Some(UserBalance {
                    user_id,
                    asset_id,
                    avail: avail as u64,
                    frozen: frozen.unwrap_or(0) as u64,
                    version: version as u64,
                    updated_at: updated_at as u64,
                }));
            }
        }

        Ok(None)
    }

    /// Get all balances for a user
    pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
        const GET_ALL_BALANCES_CQL: &str = "
            SELECT asset_id, avail, frozen, version, updated_at
            FROM user_balances
            WHERE user_id = ?
        ";

        let result = self.session.query(GET_ALL_BALANCES_CQL, (user_id as i64,)).await?;

        let mut balances = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows.into_iter() {
                let (asset_id, avail, frozen, version, updated_at): (i32, i64, i64, i64, i64) =
                    row.into_typed().context("Failed to parse balance row")?;

                balances.push(UserBalance {
                    user_id,
                    asset_id: asset_id as u32,
                    avail: avail as u64,
                    frozen: frozen as u64,
                    version: version as u64,
                    updated_at: updated_at as u64,
                });
            }
        }

        Ok(balances)
    }
    /// Update user balance for a deposit event (no version check)
    pub async fn update_balance_for_deposit(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> Result<(i64, i64, i64, i64)> {
        // Ensure record exists
        self.init_balance_if_not_exists(user_id, asset_id).await?;

        // Get current balance
        let current = self.get_user_balance(user_id, asset_id).await?;

        // Note: current is Option<UserBalance>, but we just init'd it, so likely Some.
        // But concurrent access might be tricky. Using defaults is safe.
        let current_avail = current.as_ref().map(|b| b.avail as i64).unwrap_or(0);
        let current_frozen = current.as_ref().map(|b| b.frozen as i64).unwrap_or(0);
        let current_version = current.as_ref().map(|b| b.version).unwrap_or(0);

        // Calculate new balance
        let new_avail = current_avail + amount as i64;
        let new_frozen = current_frozen; // Preserve frozen
        let new_version = current_version + 1;

        const UPDATE_BALANCE_CQL: &str = "
            UPDATE user_balances
            SET avail = ?,
                frozen = ?,
                version = ?,
                updated_at = ?
            WHERE user_id = ? AND asset_id = ?
        ";

        let now = get_current_timestamp_ms();

        self.session
            .query(
                UPDATE_BALANCE_CQL,
                (
                    new_avail,
                    new_frozen,
                    new_version as i64,
                    now,
                    user_id as i64,
                    asset_id as i32,
                ),
            )
            .await?;

        Ok((current_avail, new_avail, current_version as i64, new_version as i64))
    }

    pub async fn update_balance_for_lock(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> Result<(i64, i64, i64, i64)> {
        let current = self.get_user_balance(user_id, asset_id).await?
            .ok_or_else(|| anyhow::anyhow!("Balance not found for user {} asset {} (Lock)", user_id, asset_id))?;

        let current_avail = current.avail as i64;
        let current_frozen = current.frozen as i64;
        let current_version = current.version;

        let new_avail = current_avail - amount as i64;
        let new_frozen = current_frozen + amount as i64;
        let new_version = current_version + 1;

        const UPDATE_CQL: &str = "
            UPDATE user_balances
            SET avail = ?, frozen = ?, version = ?, updated_at = ?
            WHERE user_id = ? AND asset_id = ?
        ";
        let now = get_current_timestamp_ms();

        self.session.query(UPDATE_CQL, (
            new_avail,
            new_frozen,
            new_version as i64,
            now,
            user_id as i64,
            asset_id as i32
        )).await?;

        Ok((current_avail, new_avail, current_version as i64, new_version as i64))
    }

    pub async fn update_balance_for_unlock(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> Result<(i64, i64, i64, i64)> {
        let current = self.get_user_balance(user_id, asset_id).await?
            .ok_or_else(|| anyhow::anyhow!("Balance not found for user {} asset {} (Unlock)", user_id, asset_id))?;

        let current_avail = current.avail as i64;
        let current_frozen = current.frozen as i64;
        let current_version = current.version;

        let new_avail = current_avail + amount as i64;
        let new_frozen = current_frozen - amount as i64;
        let new_version = current_version + 1;

        const UPDATE_CQL: &str = "
            UPDATE user_balances
            SET avail = ?, frozen = ?, version = ?, updated_at = ?
            WHERE user_id = ? AND asset_id = ?
        ";
        let now = get_current_timestamp_ms();

        self.session.query(UPDATE_CQL, (
            new_avail,
            new_frozen,
            new_version as i64,
            now,
            user_id as i64,
            asset_id as i32
        )).await?;

        Ok((current_avail, new_avail, current_version as i64, new_version as i64))
    }
}

#[derive(Debug, Clone)]
pub struct UserBalance {
    pub user_id: u64,
    pub asset_id: u32,
    pub avail: u64,
    pub frozen: u64,
    pub version: u64,
    pub updated_at: u64,
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

// ========================================================================
// Atomic Settlement with LOGGED BATCH
// ========================================================================

impl SettlementDb {
    /// Settle trade atomically using sequential operations
    ///
    /// This method settles a trade by:
    /// 1. Inserting the trade into the symbol-specific table
    /// 2. Updating buyer's balances (both assets)
    /// 3. Updating seller's balances (both assets)
    ///
    /// Note: Currently uses sequential operations. Full BATCH atomicity will be added later.
    ///
    /// # Arguments
    /// * `symbol` - Trading pair symbol (e.g., "btc_usdt")
    /// * `trade` - Trade data from matching engine
    ///
    /// # Returns
    /// * `Ok(())` - Trade settled successfully
    /// * `Err(_)` - Settlement failed
    /// Helper to fetch balance with retry if version is stale
    async fn get_consistent_balance(
        &self,
        user_id: u64,
        asset_id: u32,
        expected_ver: i64,
    ) -> Result<(i64, i64, i64)> {
        let mut attempts = 0;
        loop {
            let res = self.get_user_balance(user_id, asset_id).await?;
            let (bal, frozen, ver) = match res {
                Some(b) => (b.avail as i64, b.frozen as i64, b.version as i64),
                None => (0, 0, 0),
            };

            // If version matches (or is newer), we are good.
            if ver >= expected_ver {
                return Ok((bal, frozen, ver));
            }

            attempts += 1;
            if attempts >= 5 {
                // Return whatever we have; strict check will fail later if mismatch persists
                return Ok((bal, frozen, ver));
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn settle_trade_atomically(&self, trade: &MatchExecData) -> Result<()> {
        // 1. Validate Balances Exist
        // Design Decision: We do NOT initialize balances here.
        // Users must deposit funds (which initializes the balance record) before trading.
        // If a balance is missing during settlement, it indicates a critical logic error.

        // 2. Read current balances (with retry for consistency)
        // 2. Read current balances (with retry for consistency)
        let (buyer_base, buyer_base_frozen, buyer_base_ver) = self
            .get_consistent_balance(
                trade.buyer_user_id,
                trade.base_asset,
                trade.buyer_base_version as i64,
            )
            .await?;

        let (buyer_quote, buyer_quote_frozen, buyer_quote_ver) = self
            .get_consistent_balance(
                trade.buyer_user_id,
                trade.quote_asset,
                trade.buyer_quote_version as i64,
            )
            .await?;

        let (seller_base, seller_base_frozen, seller_base_ver) = self
            .get_consistent_balance(
                trade.seller_user_id,
                trade.base_asset,
                trade.seller_base_version as i64,
            )
            .await?;

        let (seller_quote, seller_quote_frozen, seller_quote_ver) = self
            .get_consistent_balance(
                trade.seller_user_id,
                trade.quote_asset,
                trade.seller_quote_version as i64,
            )
            .await?;

        // 3. Calculate new balances
        let quote_amount = (trade.price * trade.quantity) as i64;
        let quantity = trade.quantity as i64;

        let new_buyer_base = buyer_base + quantity;
        let new_buyer_quote = buyer_quote - quote_amount;
        let new_seller_base = seller_base - quantity;
        let new_seller_quote = seller_quote + quote_amount;

        // Check data integrity: DB version must match ME version
        if buyer_base_ver != trade.buyer_base_version as i64 {
            anyhow::bail!(
                "Version mismatch Buyer Base (User {} Asset {}): DB={} ME={}",
                trade.buyer_user_id,
                trade.base_asset,
                buyer_base_ver,
                trade.buyer_base_version
            );
        }
        if buyer_quote_ver != trade.buyer_quote_version as i64 {
            anyhow::bail!(
                "Version mismatch Buyer Quote (User {} Asset {}): DB={} ME={}",
                trade.buyer_user_id,
                trade.quote_asset,
                buyer_quote_ver,
                trade.buyer_quote_version
            );
        }
        if seller_base_ver != trade.seller_base_version as i64 {
            anyhow::bail!(
                "Version mismatch Seller Base (User {} Asset {}): DB={} ME={}",
                trade.seller_user_id,
                trade.base_asset,
                seller_base_ver,
                trade.seller_base_version
            );
        }
        if seller_quote_ver != trade.seller_quote_version as i64 {
            anyhow::bail!(
                "Version mismatch Seller Quote (User {} Asset {}): DB={} ME={}",
                trade.seller_user_id,
                trade.quote_asset,
                seller_quote_ver,
                trade.seller_quote_version
            );
        }

        // Calculate new versions
        // Note: ME increments version execution steps:
        // Buyer Quote: Spend(+1) + Refund(+1 if >0)
        // Seller Base: Spend(+1) + Refund(+1 if >0)
        // Buyer Base: Deposit(+1)
        // Seller Quote: Deposit(+1)

        let buyer_quote_inc = if trade.buyer_refund > 0 { 2 } else { 1 };
        let seller_base_inc = if trade.seller_refund > 0 { 2 } else { 1 };

        let new_buyer_base_ver = buyer_base_ver + 1;
        let new_buyer_quote_ver = buyer_quote_ver + buyer_quote_inc;
        let new_seller_base_ver = seller_base_ver + seller_base_inc;
        let new_seller_quote_ver = seller_quote_ver + 1;

        // 4. Create Batch
        let mut batch = Batch::new(scylla::batch::BatchType::Logged);
        let now = get_current_timestamp_ms();
        let trade_date = get_current_date();

        // Add Trade Insert
        batch.append_statement(INSERT_TRADE_CQL);

        // Add Balance Updates
        // Add Balance Updates
        const UPDATE_BALANCE_CQL: &str = "
            UPDATE user_balances
            SET avail = ?, frozen = ?, version = ?, updated_at = ?
            WHERE user_id = ? AND asset_id = ?
        ";

        batch.append_statement(UPDATE_BALANCE_CQL); // Buyer Base
        batch.append_statement(UPDATE_BALANCE_CQL); // Buyer Quote
        batch.append_statement(UPDATE_BALANCE_CQL); // Seller Base
        batch.append_statement(UPDATE_BALANCE_CQL); // Seller Quote

        // 5. Execute Batch
        // Values for Trade Insert
        let trade_values = (
            trade_date as i32,
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
            trade.settled_at as i64,
        );

        // Values for Balance Updates
        // Values for Balance Updates
        let buyer_base_values = (
            new_buyer_base,
            buyer_base_frozen, // Preserve frozen
            new_buyer_base_ver,
            now,
            trade.buyer_user_id as i64,
            trade.base_asset as i32,
        );
        let buyer_quote_values = (
            new_buyer_quote,
            buyer_quote_frozen, // Preserve frozen
            new_buyer_quote_ver,
            now,
            trade.buyer_user_id as i64,
            trade.quote_asset as i32,
        );
        let seller_base_values = (
            new_seller_base,
            seller_base_frozen, // Preserve frozen
            new_seller_base_ver,
            now,
            trade.seller_user_id as i64,
            trade.base_asset as i32,
        );
        let seller_quote_values = (
            new_seller_quote,
            seller_quote_frozen, // Preserve frozen
            new_seller_quote_ver,
            now,
            trade.seller_user_id as i64,
            trade.quote_asset as i32,
        );

        self.session
            .batch(
                &batch,
                (
                    trade_values,
                    buyer_base_values,
                    buyer_quote_values,
                    seller_base_values,
                    seller_quote_values,
                ),
            )
            .await
            .context("Failed to execute settlement batch")?;

        log::debug!(
            "Settled trade {} (buyer={}, seller={})",
            trade.trade_id,
            trade.buyer_user_id,
            trade.seller_user_id
        );

        Ok(())
    }

    /// Check if trade already exists in symbol-specific table
    ///
    /// Used for idempotency - prevents duplicate settlement
    pub async fn trade_exists(&self, _symbol: &str, trade_id: u64) -> Result<bool> {
        // Use existing settled_trades table (per-symbol tables not yet activated)
        const QUERY: &str = "SELECT trade_id FROM settled_trades WHERE trade_id = ? LIMIT 1";

        let result = self
            .session
            .query(QUERY, (trade_id as i64,))
            .await
            .context("Failed to check if trade exists")?;

        Ok(result.rows.is_some() && !result.rows.unwrap().is_empty())
    }
}
