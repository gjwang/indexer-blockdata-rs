use anyhow::{Context, Result};
use chrono::Utc;
use scylla::batch::Batch;
use scylla::prepared_statement::PreparedStatement;
use scylla::{Session, SessionBuilder};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::common_utils::{get_current_date, get_current_timestamp_ms};
use crate::configure::ScyllaDbConfig;
use crate::ledger::{LedgerEvent, MatchExecData};
use scylla::{FromRow, SerializeRow};

#[derive(Debug, FromRow, SerializeRow)]
pub struct SettledTradeRow {
    pub trade_date: i32,
    pub output_sequence: i64,
    pub trade_id: i64,
    pub match_seq: i64,
    pub buy_order_id: i64,
    pub sell_order_id: i64,
    pub buyer_user_id: i64,
    pub seller_user_id: i64,
    pub price: i64,
    pub quantity: i64,
    pub base_asset_id: i32,
    pub quote_asset_id: i32,
    pub buyer_refund: i64,
    pub seller_refund: i64,
    pub settled_at: i64,
    pub buyer_base_version: i64,
    pub buyer_quote_version: i64,
    pub seller_base_version: i64,
    pub seller_quote_version: i64,
    pub buyer_base_balance_after: i64,
    pub buyer_quote_balance_after: i64,
    pub seller_base_balance_after: i64,
    pub seller_quote_balance_after: i64,
}

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
        price, quantity, base_asset_id, quote_asset_id,
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
        price, quantity, base_asset_id, quote_asset_id,
        buyer_refund, seller_refund, settled_at,
        buyer_base_version, buyer_quote_version,
        seller_base_version, seller_quote_version,
        buyer_base_balance_after, buyer_quote_balance_after,
        seller_base_balance_after, seller_quote_balance_after
    FROM settled_trades
    WHERE trade_id = ?
";

const SELECT_TRADES_BY_SEQ_CQL: &str = "
    SELECT
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset_id, quote_asset_id,
        buyer_refund, seller_refund, settled_at,
        buyer_base_version, buyer_quote_version,
        seller_base_version, seller_quote_version,
        buyer_base_balance_after, buyer_quote_balance_after,
        seller_base_balance_after, seller_quote_balance_after
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
        price, quantity, base_asset_id, quote_asset_id,
        buyer_refund, seller_refund, settled_at,
        buyer_base_version, buyer_quote_version,
        seller_base_version, seller_quote_version,
        buyer_base_balance_after, buyer_quote_balance_after,
        seller_base_balance_after, seller_quote_balance_after
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
        price, quantity, base_asset_id, quote_asset_id,
        buyer_refund, seller_refund, settled_at,
        buyer_base_version, buyer_quote_version,
        seller_base_version, seller_quote_version,
        buyer_base_balance_after, buyer_quote_balance_after,
        seller_base_balance_after, seller_quote_balance_after
    FROM settled_trades
    WHERE seller_user_id = ?
    LIMIT ?
    ALLOW FILTERING
";

// === APPEND-ONLY BALANCE LEDGER ===
// Each row is immutable - insert only, never update
// Current balance = latest row with highest seq

/// Insert a new balance event (append-only, idempotent with IF NOT EXISTS)
const INSERT_BALANCE_EVENT_CQL: &str = "
    INSERT INTO balance_ledger (
        user_id, asset_id, seq,
        delta_avail, delta_frozen,
        avail, frozen,
        event_type, ref_id, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    IF NOT EXISTS
";

/// Get latest balance (highest seq) for a user/asset pair
const SELECT_LATEST_BALANCE_CQL: &str = "
    SELECT seq, avail, frozen, created_at
    FROM balance_ledger
    WHERE user_id = ? AND asset_id = ?
    LIMIT 1
";

/// Get balance history for a user/asset (most recent first)
const SELECT_BALANCE_HISTORY_CQL: &str = "
    SELECT seq, delta_avail, delta_frozen, avail, frozen, event_type, ref_id, created_at
    FROM balance_ledger
    WHERE user_id = ? AND asset_id = ?
    LIMIT ?
";

/// Balance event types
pub mod balance_event_types {
    pub const DEPOSIT: &str = "deposit";
    pub const WITHDRAW: &str = "withdraw";
    pub const LOCK: &str = "lock";
    pub const UNLOCK: &str = "unlock";
    pub const TRADE_DEBIT: &str = "trade_debit";
    pub const TRADE_CREDIT: &str = "trade_credit";
    pub const REFUND: &str = "refund";
}

/// Balance ledger entry (append-only row)
#[derive(Debug, Clone, FromRow)]
pub struct BalanceLedgerEntry {
    pub user_id: i64,
    pub asset_id: i32,
    pub seq: i64,
    pub delta_avail: i64,
    pub delta_frozen: i64,
    pub avail: i64,
    pub frozen: i64,
    pub event_type: String,
    pub ref_id: i64,
    pub created_at: i64,
}

/// Current balance snapshot (from latest ledger entry)
#[derive(Debug, Clone, Default)]
pub struct CurrentBalance {
    pub seq: i64,
    pub avail: i64,
    pub frozen: i64,
    pub updated_at: i64,
}

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
                        trade.base_asset_id as i32,
                        trade.quote_asset_id as i32,
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
                let row = SettledTradeRow {
                    trade_date,
                    output_sequence: trade.output_sequence as i64,
                    trade_id: trade.trade_id as i64,
                    match_seq: trade.match_seq as i64,
                    buy_order_id: trade.buy_order_id as i64,
                    sell_order_id: trade.sell_order_id as i64,
                    buyer_user_id: trade.buyer_user_id as i64,
                    seller_user_id: trade.seller_user_id as i64,
                    price: trade.price as i64,
                    quantity: trade.quantity as i64,
                    base_asset_id: trade.base_asset_id as i32,
                    quote_asset_id: trade.quote_asset_id as i32,
                    buyer_refund: trade.buyer_refund as i64,
                    seller_refund: trade.seller_refund as i64,
                    settled_at,
                    buyer_base_version: trade.buyer_base_version as i64,
                    buyer_quote_version: trade.buyer_quote_version as i64,
                    seller_base_version: trade.seller_base_version as i64,
                    seller_quote_version: trade.seller_quote_version as i64,
                    buyer_base_balance_after: trade.buyer_base_balance_after as i64,
                    buyer_quote_balance_after: trade.buyer_quote_balance_after as i64,
                    seller_base_balance_after: trade.seller_base_balance_after as i64,
                    seller_quote_balance_after: trade.seller_quote_balance_after as i64,
                };
                self.session.execute(&self.insert_trade_stmt, row).await.map(|_| ())
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

    /// Insert trades in a single ScyllaDB BATCH (much faster than sequential)
    pub async fn insert_trades_batch(
        &self,
        trades: &[crate::engine_output::TradeOutput],
        output_seq: u64,
    ) -> Result<()> {
        if trades.is_empty() {
            return Ok(());
        }

        use scylla::batch::BatchType;
        let mut batch = Batch::new(BatchType::Unlogged);

        let trade_date = get_current_date();
        let settled_at = get_current_timestamp_ms();

        // Add statement for each trade
        for _ in trades {
            batch.append_statement(INSERT_TRADE_CQL);
        }

        // Build values
        let values: Vec<_> = trades
            .iter()
            .map(|t| {
                (
                    trade_date,
                    output_seq as i64,
                    t.trade_id as i64,
                    t.match_seq as i64,
                    t.buy_order_id as i64,
                    t.sell_order_id as i64,
                    t.buyer_user_id as i64,
                    t.seller_user_id as i64,
                    t.price as i64,
                    t.quantity as i64,
                    t.base_asset_id as i32,
                    t.quote_asset_id as i32,
                    t.buyer_refund as i64,
                    t.seller_refund as i64,
                    if t.settled_at > 0 { t.settled_at as i64 } else { settled_at },
                )
            })
            .collect();

        self.session.batch(&batch, values).await.context("Failed to batch insert trades")?;

        log::debug!("Batch inserted {} trades", trades.len());
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
        let r: SettledTradeRow =
            row.into_typed().context("Failed to parse row into SettledTradeRow")?;

        Ok(MatchExecData {
            trade_id: r.trade_id as u64,
            buy_order_id: r.buy_order_id as u64,
            sell_order_id: r.sell_order_id as u64,
            buyer_user_id: r.buyer_user_id as u64,
            seller_user_id: r.seller_user_id as u64,
            price: r.price as u64,
            quantity: r.quantity as u64,
            base_asset_id: r.base_asset_id as u32,
            quote_asset_id: r.quote_asset_id as u32,
            buyer_refund: r.buyer_refund as u64,
            seller_refund: r.seller_refund as u64,
            match_seq: r.match_seq as u64,
            output_sequence: r.output_sequence as u64,
            settled_at: r.settled_at as u64,
            buyer_quote_version: r.buyer_quote_version as u64,
            buyer_base_version: r.buyer_base_version as u64,
            seller_base_version: r.seller_base_version as u64,
            seller_quote_version: r.seller_quote_version as u64,
            buyer_quote_balance_after: r.buyer_quote_balance_after as u64,
            buyer_base_balance_after: r.buyer_base_balance_after as u64,
            seller_base_balance_after: r.seller_base_balance_after as u64,
            seller_quote_balance_after: r.seller_quote_balance_after as u64,
        })
    }

    /// Get all balances for a user (from balance_ledger - latest entry per asset)
    pub async fn get_user_all_balances(&self, user_id: u64) -> Result<Vec<UserBalance>> {
        // Query all distinct assets for this user, getting latest seq per asset
        const QUERY: &str = "
            SELECT asset_id, avail, frozen, seq, created_at
            FROM balance_ledger
            WHERE user_id = ?
        ";

        let result = self.session.query(QUERY, (user_id as i64,)).await?;

        // Group by asset_id, keep only latest seq
        let mut latest: std::collections::HashMap<i32, (i64, i64, i64, i64, i64)> =
            std::collections::HashMap::new();

        if let Some(rows) = result.rows {
            for row in rows.into_iter() {
                let (asset_id, avail, frozen, seq, created_at): (i32, i64, i64, i64, i64) =
                    row.into_typed().context("Failed to parse balance row")?;

                // Keep latest seq per asset
                let entry = latest.entry(asset_id).or_insert((seq, avail, frozen, seq, created_at));
                if seq > entry.0 {
                    *entry = (seq, avail, frozen, seq, created_at);
                }
            }
        }

        let balances: Vec<UserBalance> = latest
            .into_iter()
            .map(|(asset_id, (_, avail, frozen, version, updated_at))| UserBalance {
                user_id,
                asset_id: asset_id as u32,
                avail: avail as u64,
                frozen: frozen as u64,
                version: version as u64,
                updated_at: updated_at as u64,
            })
            .collect();

        Ok(balances)
    }

    // =====================================================
    // APPEND-ONLY BALANCE LEDGER METHODS
    // =====================================================

    /// Get current balance from append-only ledger (latest entry)
    /// Returns None if no balance exists (user hasn't deposited yet)
    pub async fn get_current_balance(
        &self,
        user_id: u64,
        asset_id: u32,
    ) -> Result<Option<CurrentBalance>> {
        let result = self
            .session
            .query(SELECT_LATEST_BALANCE_CQL, (user_id as i64, asset_id as i32))
            .await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                let (seq, avail, frozen, created_at): (i64, i64, i64, i64) =
                    row.into_typed().context("Failed to parse balance row")?;
                return Ok(Some(CurrentBalance { seq, avail, frozen, updated_at: created_at }));
            }
        }

        // No balance exists - user must deposit first
        Ok(None)
    }

    /// Append a balance event to the ledger (immutable insert)
    /// Also updates user_balances to reflect current state
    pub async fn append_balance_event(
        &self,
        user_id: u64,
        asset_id: u32,
        seq: u64,
        delta_avail: i64,
        delta_frozen: i64,
        avail: i64,
        frozen: i64,
        event_type: &str,
        ref_id: u64,
    ) -> Result<()> {
        let now = get_current_timestamp_ms();

        // 1. Append to balance_ledger (immutable log)
        self.session
            .query(
                INSERT_BALANCE_EVENT_CQL,
                (
                    user_id as i64,
                    asset_id as i32,
                    seq as i64,
                    delta_avail,
                    delta_frozen,
                    avail,
                    frozen,
                    event_type,
                    ref_id as i64,
                    now,
                ),
            )
            .await
            .context("Failed to insert balance event")?;

        // 2. Update user_balances snapshot table for easy querying
        log::info!(
            "Updating user_balances: user={} asset={} avail={} frozen={} version={}",
            user_id,
            asset_id,
            avail,
            frozen,
            seq
        );

        if let Err(e) = self.session
            .query(
                "INSERT INTO user_balances (user_id, asset_id, avail, frozen, version, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                (user_id as i64, asset_id as i32, avail, frozen, seq as i64, now as i64),
            )
            .await
        {
            log::error!("Failed to update user_balances: user={} asset={}: {}", user_id, asset_id, e);
            return Err(e.into());
        }

        Ok(())
    }

    /// Batch append balance events (much faster than individual calls)
    /// Uses ScyllaDB BATCH for single round-trip
    pub async fn append_balance_events_batch(
        &self,
        events: &[crate::engine_output::BalanceEvent],
    ) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        use scylla::batch::BatchType;
        let mut batch = Batch::new(BatchType::Unlogged);
        let now = get_current_timestamp_ms();

        // Add all balance_ledger inserts and user_balances updates to batch
        for event in events {
            // balance_ledger insert
            batch.append_statement(INSERT_BALANCE_EVENT_CQL);
            // user_balances update
            batch.append_statement(
                "INSERT INTO user_balances (user_id, asset_id, avail, frozen, version, updated_at) VALUES (?, ?, ?, ?, ?, ?)"
            );
        }

        // Build values for all statements
        let mut values: Vec<(i64, i32, i64, i64, i64, i64, i64, String, i64, i64)> =
            Vec::with_capacity(events.len());
        let mut user_values: Vec<(i64, i32, i64, i64, i64, i64)> = Vec::with_capacity(events.len());

        for event in events {
            values.push((
                event.user_id as i64,
                event.asset_id as i32,
                event.seq as i64,
                event.delta_avail,
                event.delta_frozen,
                event.avail,
                event.frozen,
                event.event_type.clone(),
                event.ref_id as i64,
                now as i64,
            ));
            user_values.push((
                event.user_id as i64,
                event.asset_id as i32,
                event.avail,
                event.frozen,
                event.seq as i64,
                now as i64,
            ));
        }

        // Execute alternating batch (ledger, user, ledger, user, ...)
        // Actually, ScyllaDB batch with different statement types is tricky
        // Let's use a simpler approach: batch all ledger, then batch all user_balances

        let mut ledger_batch = Batch::new(BatchType::Unlogged);
        // Note: Can't use IF NOT EXISTS (LWT) in batch - idempotency ensured by primary key
        let ledger_stmt = "INSERT INTO balance_ledger (user_id, asset_id, seq, delta_avail, delta_frozen, avail, frozen, event_type, ref_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        for _ in events {
            ledger_batch.append_statement(ledger_stmt);
        }

        let ledger_values: Vec<_> = events
            .iter()
            .map(|e| {
                (
                    e.user_id as i64,
                    e.asset_id as i32,
                    e.seq as i64,
                    e.delta_avail,
                    e.delta_frozen,
                    e.avail,
                    e.frozen,
                    e.event_type.clone(),
                    e.ref_id as i64,
                    now as i64,
                )
            })
            .collect();

        self.session
            .batch(&ledger_batch, ledger_values)
            .await
            .context("Failed to batch insert balance_ledger")?;

        // Update user_balances SNAPSHOT - only write FINAL state per (user_id, asset_id)
        // This is efficient: instead of writing every event, we just write the last one
        use std::collections::HashMap;
        let mut latest: HashMap<(u64, u32), &crate::engine_output::BalanceEvent> = HashMap::new();
        for event in events {
            let key = (event.user_id, event.asset_id);
            match latest.get(&key) {
                Some(existing) if existing.seq >= event.seq => {} // Keep existing (higher seq)
                _ => { latest.insert(key, event); }
            }
        }

        if !latest.is_empty() {
            let mut user_batch = Batch::new(BatchType::Unlogged);
            let user_stmt = "INSERT INTO user_balances (user_id, asset_id, avail, frozen, version, updated_at) VALUES (?, ?, ?, ?, ?, ?)";
            for _ in 0..latest.len() {
                user_batch.append_statement(user_stmt);
            }

            let user_values: Vec<_> = latest
                .values()
                .map(|e| {
                    (e.user_id as i64, e.asset_id as i32, e.avail, e.frozen, e.seq as i64, now as i64)
                })
                .collect();

            self.session
                .batch(&user_batch, user_values)
                .await
                .context("Failed to batch update user_balances snapshot")?;
        }

        log::debug!("Batch processed {} balance events ({} user snapshots)", events.len(), latest.len());
        Ok(())
    }

    /// Deposit - append-only version (v2)
    pub async fn deposit(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        new_seq: u64,
        ref_id: u64,
    ) -> Result<CurrentBalance> {
        // Get current balance (None = first deposit)
        let current = self.get_current_balance(user_id, asset_id).await?.unwrap_or_default();

        // Idempotency check
        if new_seq <= current.seq as u64 {
            return Ok(current);
        }

        // Calculate new balance
        let delta_avail = amount as i64;
        let new_avail = current.avail + delta_avail;
        let new_frozen = current.frozen; // Deposit doesn't change frozen

        // Append event
        self.append_balance_event(
            user_id,
            asset_id,
            new_seq,
            delta_avail,
            0, // delta_frozen
            new_avail,
            new_frozen,
            balance_event_types::DEPOSIT,
            ref_id,
        )
        .await?;

        Ok(CurrentBalance {
            seq: new_seq as i64,
            avail: new_avail,
            frozen: new_frozen,
            updated_at: get_current_timestamp_ms() as i64,
        })
    }

    /// Lock - append-only version (v2)
    pub async fn lock(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        new_seq: u64,
        ref_id: u64,
    ) -> Result<CurrentBalance> {
        let current = self.get_current_balance(user_id, asset_id).await?.ok_or_else(|| {
            anyhow::anyhow!(
                "No balance for user {} asset {} - deposit required first",
                user_id,
                asset_id
            )
        })?;

        if new_seq <= current.seq as u64 {
            return Ok(current);
        }

        let delta_avail = -(amount as i64);
        let delta_frozen = amount as i64;
        let new_avail = current.avail + delta_avail;
        let new_frozen = current.frozen + delta_frozen;

        self.append_balance_event(
            user_id,
            asset_id,
            new_seq,
            delta_avail,
            delta_frozen,
            new_avail,
            new_frozen,
            balance_event_types::LOCK,
            ref_id,
        )
        .await?;

        Ok(CurrentBalance {
            seq: new_seq as i64,
            avail: new_avail,
            frozen: new_frozen,
            updated_at: get_current_timestamp_ms() as i64,
        })
    }

    /// Unlock - append-only version (v2)
    pub async fn unlock(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        new_seq: u64,
        ref_id: u64,
    ) -> Result<CurrentBalance> {
        let current = self.get_current_balance(user_id, asset_id).await?.ok_or_else(|| {
            anyhow::anyhow!(
                "No balance for user {} asset {} - deposit required first",
                user_id,
                asset_id
            )
        })?;

        if new_seq <= current.seq as u64 {
            return Ok(current);
        }

        let delta_avail = amount as i64;
        let delta_frozen = -(amount as i64);
        let new_avail = current.avail + delta_avail;
        let new_frozen = current.frozen + delta_frozen;

        self.append_balance_event(
            user_id,
            asset_id,
            new_seq,
            delta_avail,
            delta_frozen,
            new_avail,
            new_frozen,
            balance_event_types::UNLOCK,
            ref_id,
        )
        .await?;

        Ok(CurrentBalance {
            seq: new_seq as i64,
            avail: new_avail,
            frozen: new_frozen,
            updated_at: get_current_timestamp_ms() as i64,
        })
    }

    /// Withdraw - append-only version (v2)
    pub async fn withdraw(
        &self,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        new_seq: u64,
        ref_id: u64,
    ) -> Result<CurrentBalance> {
        let current = self.get_current_balance(user_id, asset_id).await?.ok_or_else(|| {
            anyhow::anyhow!(
                "No balance for user {} asset {} - deposit required first",
                user_id,
                asset_id
            )
        })?;

        if new_seq <= current.seq as u64 {
            return Ok(current);
        }

        let delta_avail = -(amount as i64);
        let new_avail = current.avail + delta_avail;
        let new_frozen = current.frozen;

        self.append_balance_event(
            user_id,
            asset_id,
            new_seq,
            delta_avail,
            0,
            new_avail,
            new_frozen,
            balance_event_types::WITHDRAW,
            ref_id,
        )
        .await?;

        Ok(CurrentBalance {
            seq: new_seq as i64,
            avail: new_avail,
            frozen: new_frozen,
            updated_at: get_current_timestamp_ms() as i64,
        })
    }

    /// Get balance history (most recent first)
    pub async fn get_balance_history(
        &self,
        user_id: u64,
        asset_id: u32,
        limit: u32,
    ) -> Result<Vec<BalanceLedgerEntry>> {
        let result = self
            .session
            .query(SELECT_BALANCE_HISTORY_CQL, (user_id as i64, asset_id as i32, limit as i32))
            .await?;

        let mut entries = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                let (seq, delta_avail, delta_frozen, avail, frozen, event_type, ref_id, created_at):
                    (i64, i64, i64, i64, i64, String, i64, i64) = row.into_typed()?;
                entries.push(BalanceLedgerEntry {
                    user_id: user_id as i64,
                    asset_id: asset_id as i32,
                    seq,
                    delta_avail,
                    delta_frozen,
                    avail,
                    frozen,
                    event_type,
                    ref_id,
                    created_at,
                });
            }
        }

        Ok(entries)
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
            // Use new append-only balance ledger
            let current = self.get_current_balance(user_id, asset_id).await?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Balance not found for user {} asset {} - deposit required first",
                    user_id,
                    asset_id
                )
            })?;
            let (bal, frozen, ver) = (current.avail, current.frozen, current.seq);

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
        // 0. Check if trade already exists (idempotency check)
        let check_query =
            "SELECT trade_id FROM settled_trades WHERE trade_date = ? AND trade_id = ?";
        let trade_date = get_current_date();
        let existing =
            self.session.query(check_query, (trade_date as i32, trade.trade_id as i64)).await;

        if let Ok(result) = existing {
            if result.rows_num()? > 0 {
                log::warn!("Trade already exists in settled_trades: trade={}", trade.trade_id);
                return Ok(());
            }
        }

        // 1. Validate Balances Exist
        // Design Decision: We do NOT initialize balances here.
        // Users must deposit funds (which initializes the balance record) before trading.
        // If a balance is missing during settlement, it indicates a critical logic error.

        // 2. Read current balances (with retry for consistency)
        // 2. Read current balances (with retry for consistency)
        let (buyer_base, buyer_base_frozen, buyer_base_ver) = self
            .get_consistent_balance(
                trade.buyer_user_id,
                trade.base_asset_id,
                trade.buyer_base_version as i64,
            )
            .await?;

        let (buyer_quote, buyer_quote_frozen, buyer_quote_ver) = self
            .get_consistent_balance(
                trade.buyer_user_id,
                trade.quote_asset_id,
                trade.buyer_quote_version as i64,
            )
            .await?;

        let (seller_base, seller_base_frozen, seller_base_ver) = self
            .get_consistent_balance(
                trade.seller_user_id,
                trade.base_asset_id,
                trade.seller_base_version as i64,
            )
            .await?;

        let (seller_quote, seller_quote_frozen, seller_quote_ver) = self
            .get_consistent_balance(
                trade.seller_user_id,
                trade.quote_asset_id,
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

        // Check data integrity: DB version must be predecessor of ME version (ME - 1)
        // Since trade execution increments version by 1.
        if buyer_base_ver >= trade.buyer_base_version as i64 {
            if buyer_base_ver == trade.buyer_base_version as i64 {
                // Version matches - this can happen if Lock was already applied
                // Trade existence was already checked, so we can proceed
                log::debug!(
                    "Version matches ME (Lock may have been applied): trade={}, buyer_base_ver={}",
                    trade.trade_id,
                    buyer_base_ver
                );
            } else {
                anyhow::bail!(
                    "Version mismatch Buyer Base (DB ahead?): DB={} ME={}",
                    buyer_base_ver,
                    trade.buyer_base_version
                );
            }
        } else if buyer_base_ver != (trade.buyer_base_version as i64 - 1) {
            // Gap detected
            anyhow::bail!(
                "Version gap Buyer Base: DB={} ME={}",
                buyer_base_ver,
                trade.buyer_base_version
            );
        }

        if buyer_quote_ver >= trade.buyer_quote_version as i64 {
            if buyer_quote_ver != trade.buyer_quote_version as i64 {
                anyhow::bail!(
                    "Version mismatch Buyer Quote (DB ahead?): DB={} ME={}",
                    buyer_quote_ver,
                    trade.buyer_quote_version
                );
            }
        } else if buyer_quote_ver != (trade.buyer_quote_version as i64 - 1) {
            anyhow::bail!(
                "Version gap Buyer Quote: DB={} ME={}",
                buyer_quote_ver,
                trade.buyer_quote_version
            );
        }

        if seller_base_ver >= trade.seller_base_version as i64 {
            if seller_base_ver != trade.seller_base_version as i64 {
                anyhow::bail!(
                    "Version mismatch Seller Base (DB ahead?): DB={} ME={}",
                    seller_base_ver,
                    trade.seller_base_version
                );
            }
        } else if seller_base_ver != (trade.seller_base_version as i64 - 1) {
            anyhow::bail!(
                "Version gap Seller Base: DB={} ME={}",
                seller_base_ver,
                trade.seller_base_version
            );
        }

        if seller_quote_ver >= trade.seller_quote_version as i64 {
            if seller_quote_ver != trade.seller_quote_version as i64 {
                anyhow::bail!(
                    "Version mismatch Seller Quote (DB ahead?): DB={} ME={}",
                    seller_quote_ver,
                    trade.seller_quote_version
                );
            }
        } else if seller_quote_ver != (trade.seller_quote_version as i64 - 1) {
            anyhow::bail!(
                "Version gap Seller Quote: DB={} ME={}",
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

        // 4. Prepare Trade Insert
        let _now = get_current_timestamp_ms();
        let trade_date = get_current_date();

        // Values for Trade Insert (15 columns - within tuple limit)
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
            trade.base_asset_id as i32,
            trade.quote_asset_id as i32,
            trade.buyer_refund as i64,
            trade.seller_refund as i64,
            trade.settled_at as i64,
        );

        // 5. Execute Settlement
        // Note: We split this into 2 operations because trade_values tuple (23 elements)
        // exceeds Scylla driver's 16-element SerializeRow limit

        // 5.1 Insert Trade
        self.session.query(INSERT_TRADE_CQL, trade_values).await.map_err(|e| {
            log::error!("Trade insert failed: {:?}", e);
            anyhow::anyhow!("Failed to insert trade: {}", e)
        })?;

        // 5.2 Append balance events to ledger (append-only)
        let trade_ref_id = trade.trade_id;

        // Buyer Base: Receives base asset
        self.append_balance_event(
            trade.buyer_user_id,
            trade.base_asset_id,
            new_buyer_base_ver as u64,
            quantity,
            0, // No change to frozen for receiving
            new_buyer_base,
            buyer_base_frozen, // Keep frozen as is
            balance_event_types::TRADE_CREDIT,
            trade_ref_id,
        )
        .await?;

        // Buyer Quote: Spent quote asset (from frozen)
        self.append_balance_event(
            trade.buyer_user_id,
            trade.quote_asset_id,
            new_buyer_quote_ver as u64,
            0,             // avail doesn't change (was already locked)
            -quote_amount, // frozen decreases
            new_buyer_quote,
            buyer_quote_frozen - quote_amount, // Update frozen
            balance_event_types::TRADE_DEBIT,
            trade_ref_id,
        )
        .await?;

        // Seller Base: Spent base asset (from frozen)
        self.append_balance_event(
            trade.seller_user_id,
            trade.base_asset_id,
            new_seller_base_ver as u64,
            0,         // avail doesn't change (was already locked)
            -quantity, // frozen decreases
            new_seller_base,
            seller_base_frozen - quantity, // Update frozen
            balance_event_types::TRADE_DEBIT,
            trade_ref_id,
        )
        .await?;

        // Seller Quote: Receives quote asset
        self.append_balance_event(
            trade.seller_user_id,
            trade.quote_asset_id,
            new_seller_quote_ver as u64,
            quote_amount, // avail increases
            0,            // frozen stays same
            new_seller_quote,
            seller_quote_frozen,
            balance_event_types::TRADE_CREDIT,
            trade_ref_id,
        )
        .await?;

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

    /// Get the last processed chain state for crash recovery
    /// Reads from engine_output_log (the source of truth) instead of a separate state table
    pub async fn get_chain_state(&self) -> Result<(u64, u64)> {
        const QUERY: &str =
            "SELECT output_seq, hash FROM engine_output_log ORDER BY output_seq DESC LIMIT 1";

        let result = self
            .session
            .query(QUERY, &[])
            .await
            .context("Failed to query engine_output_log for chain state")?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                let seq: i64 = row.columns[0].as_ref().and_then(|v| v.as_bigint()).unwrap_or(0);
                let hash: i64 = row.columns[1].as_ref().and_then(|v| v.as_bigint()).unwrap_or(0);
                return Ok((seq as u64, hash as u64));
            }
        }

        Ok((0, 0)) // No state found, start from beginning
    }

    /// Save chain state after successful processing
    pub async fn save_chain_state(&self, last_output_seq: u64, last_hash: u64) -> Result<()> {
        const QUERY: &str = "INSERT INTO settlement_state (partition_key, last_output_seq, last_hash, updated_at) VALUES ('main', ?, ?, ?)";

        let now = get_current_timestamp_ms();
        self.session
            .query(QUERY, (last_output_seq as i64, last_hash as i64, now as i64))
            .await
            .context("Failed to save settlement_state")?;

        Ok(())
    }

    /// Write EngineOutput to log (Source of Truth)
    /// This is the single source of truth - chain state can be recovered by reading the latest entry
    pub async fn write_engine_output(
        &self,
        output: &crate::engine_output::EngineOutput,
    ) -> Result<()> {
        const QUERY: &str = "INSERT INTO engine_output_log (output_seq, hash, prev_hash, output_data, created_at) VALUES (?, ?, ?, ?, ?)";

        let output_bytes =
            serde_json::to_vec(output).context("Failed to serialize EngineOutput")?;
        let now = get_current_timestamp_ms();

        self.session
            .query(
                QUERY,
                (
                    output.output_seq as i64,
                    output.hash as i64,
                    output.prev_hash as i64,
                    output_bytes,
                    now,
                ),
            )
            .await
            .context("Failed to write engine_output_log")?;

        Ok(())
    }

    /// Batch write multiple EngineOutputs to log (Source of Truth) - ULTRA FAST
    /// Writes all outputs in a single ScyllaDB batch for maximum throughput
    pub async fn write_engine_outputs_batch(
        &self,
        outputs: &[crate::engine_output::EngineOutput],
    ) -> Result<()> {
        if outputs.is_empty() {
            return Ok(());
        }

        use scylla::batch::Batch;
        use scylla::batch::BatchType;

        const QUERY: &str = "INSERT INTO engine_output_log (output_seq, hash, prev_hash, output_data, created_at) VALUES (?, ?, ?, ?, ?)";

        let mut batch = Batch::new(BatchType::Unlogged);
        let mut values = Vec::with_capacity(outputs.len());
        let now = get_current_timestamp_ms();

        let mut total_bytes = 0usize;
        let mut min_bytes = usize::MAX;
        let mut max_bytes = 0usize;

        for output in outputs {
            let output_bytes =
                serde_json::to_vec(output).context("Failed to serialize EngineOutput")?;

            let size = output_bytes.len();
            total_bytes += size;
            min_bytes = min_bytes.min(size);
            max_bytes = max_bytes.max(size);

            batch.append_statement(QUERY);
            values.push((
                output.output_seq as i64,
                output.hash as i64,
                output.prev_hash as i64,
                output_bytes,
                now,
            ));
        }

        let avg_bytes = total_bytes / outputs.len().max(1);
        log::info!(
            "SOT batch: {} outputs, total={}KB, avg={}B, min={}B, max={}B",
            outputs.len(),
            total_bytes / 1024,
            avg_bytes,
            min_bytes,
            max_bytes
        );

        self.session
            .batch(&batch, values)
            .await
            .context("Failed to batch write engine_output_log")?;

        Ok(())
    }

    /// Check if EngineOutput already exists in log (for idempotency)
    pub async fn engine_output_exists(&self, output_seq: u64) -> Result<bool> {
        const QUERY: &str = "SELECT output_seq FROM engine_output_log WHERE output_seq = ? LIMIT 1";

        let result = self
            .session
            .query(QUERY, (output_seq as i64,))
            .await
            .context("Failed to check engine_output_log")?;

        Ok(result.rows.is_some() && !result.rows.unwrap().is_empty())
    }
}
