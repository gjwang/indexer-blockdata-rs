use anyhow::Result;
use scylla::{Session, SessionBuilder};
use std::sync::Arc;

use crate::configure::ScyllaDbConfig;
use crate::ledger::{OrderStatus, OrderUpdate};

/// Order History Database Repository
/// Manages all ScyllaDB operations for order history tracking
#[derive(Clone)]
pub struct OrderHistoryDb {
    session: Arc<Session>,
    keyspace: String,
}

impl OrderHistoryDb {
    /// Connect to ScyllaDB
    pub async fn connect(config: &ScyllaDbConfig) -> Result<Self> {
        let builder = SessionBuilder::new().known_nodes(&config.hosts);

        let session = builder.build().await?;
        session.use_keyspace(&config.keyspace, false).await?;

        Ok(Self {
            session: Arc::new(session),
            keyspace: config.keyspace.clone(),
        })
    }

    /// Health check
    pub async fn health_check(&self) -> Result<bool> {
        let query = "SELECT now() FROM system.local";
        self.session.query(query, &[]).await?;
        Ok(true)
    }

    // ========================================================================
    // Active Orders Operations
    // ========================================================================

    /// Insert or update an active order
    pub async fn upsert_active_order(&self, order: &OrderUpdate) -> Result<()> {
        let query = "
            INSERT INTO active_orders (
                user_id, order_id, client_order_id, symbol, side, order_type,
                price, qty, filled_qty, avg_fill_price, status, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ";

        self.session
            .query(
                query,
                (
                    order.user_id as i64,
                    order.order_id as i64,
                    order.client_order_id.as_deref().unwrap_or(""),
                    &order.symbol,
                    &order.side,
                    &order.order_type,
                    order.price as i64,
                    order.qty as i64,
                    order.filled_qty as i64,
                    order.avg_fill_price.unwrap_or(0) as i64,
                    format!("{:?}", order.status),
                    order.timestamp as i64,
                    order.timestamp as i64,
                ),
            )
            .await?;

        Ok(())
    }

    /// Delete an active order (when filled or cancelled)
    pub async fn delete_active_order(&self, user_id: u64, order_id: u64) -> Result<()> {
        let query = "DELETE FROM active_orders WHERE user_id = ? AND order_id = ?";

        self.session
            .query(query, (user_id as i64, order_id as i64))
            .await?;

        Ok(())
    }

    /// Get active order state
    pub async fn get_active_order(&self, user_id: u64, order_id: u64) -> Result<Option<OrderUpdate>> {
            let query = "SELECT user_id, order_id, client_order_id, symbol, price, qty, filled_qty, status, created_at, side, order_type FROM active_orders WHERE user_id = ? AND order_id = ?";
        let result = self.session.query(query, (user_id as i64, order_id as i64)).await?;

        if let Some(rows) = result.rows {
             for row in rows {
                 if let Ok((uid, oid, cid, sym, price, qty, filled, status_str, ts, side, type_str)) = row.into_typed::<(i64, i64, String, String, i64, i64, i64, String, i64, String, String)>() {
                     let status = match status_str.as_str() {
                         "New" => OrderStatus::New,
                         "PartiallyFilled" => OrderStatus::PartiallyFilled,
                         "Filled" => OrderStatus::Filled,
                         _ => OrderStatus::New,
                     };

                     return Ok(Some(OrderUpdate {
                         order_id: oid as u64,
                         client_order_id: if cid.is_empty() { None } else { Some(cid) },
                         user_id: uid as u64,
                         symbol: sym,
                         side,
                         order_type: type_str,
                         status,
                         price: price as u64,
                         qty: qty as u64,
                         filled_qty: filled as u64,
                         avg_fill_price: None,
                         rejection_reason: None,
                         timestamp: ts as u64,
                         match_id: None,
                     }));
                 }
             }
        }
        Ok(None)
    }

    // ========================================================================
    // Order History Operations
    // ========================================================================

    /// Insert order history record
    pub async fn insert_order_history(&self, order: &OrderUpdate) -> Result<()> {
        let query = "
            INSERT INTO order_history (
                user_id, created_at, order_id, client_order_id, symbol, side, order_type,
                price, qty, filled_qty, avg_fill_price, status, rejection_reason,
                match_id, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ";

        self.session
            .query(
                query,
                (
                    order.user_id as i64,
                    order.timestamp as i64,
                    order.order_id as i64,
                    order.client_order_id.as_deref().unwrap_or(""),
                    &order.symbol,
                    &order.side,
                    &order.order_type,
                    order.price as i64,
                    order.qty as i64,
                    order.filled_qty as i64,
                    order.avg_fill_price.unwrap_or(0) as i64,
                    format!("{:?}", order.status),
                    order.rejection_reason.as_deref().unwrap_or(""),
                    order.match_id.map(|id| id as i64),
                    order.timestamp as i64,
                ),
            )
            .await?;

        Ok(())
    }

    // ========================================================================
    // Order Updates Stream Operations
    // ========================================================================

    /// Insert order update event into stream
    pub async fn insert_order_update_stream(&self, order: &OrderUpdate, event_id: u64) -> Result<()> {
        // Calculate event_date (Days since epoch)
        let event_date = (order.timestamp / 8640_0000) as i32;

        let query = "
            INSERT INTO order_updates_stream (
                event_date, event_id, order_id, user_id, client_order_id, symbol,
                status, price, qty, filled_qty, avg_fill_price, rejection_reason,
                match_id, timestamp
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ";

        self.session
            .query(
                query,
                (
                    event_date,
                    event_id as i64,
                    order.order_id as i64,
                    order.user_id as i64,
                    order.client_order_id.as_deref().unwrap_or(""),
                    &order.symbol,
                    format!("{:?}", order.status),
                    order.price as i64,
                    order.qty as i64,
                    order.filled_qty as i64,
                    order.avg_fill_price.unwrap_or(0) as i64,
                    order.rejection_reason.as_deref().unwrap_or(""),
                    order.match_id.map(|id| id as i64),
                    order.timestamp as i64,
                ),
            )
            .await?;

        Ok(())
    }

    // ========================================================================
    // Order Statistics Operations
    // ========================================================================

    /// Update order statistics for a user
    pub async fn update_order_statistics(&self, user_id: u64, status: &OrderStatus, timestamp: u64) -> Result<()> {
        // 1. Fetch current statistics
        let select_query = "SELECT total_orders, filled_orders, cancelled_orders, rejected_orders FROM order_statistics WHERE user_id = ?";
        let current_stats = self.session.query(select_query, (user_id as i64,)).await?;

        let (mut total, mut filled, mut cancelled, mut rejected) = if let Some(rows) = current_stats.rows {
            if let Some(row) = rows.into_iter().next() {
                row.into_typed::<(i32, i32, i32, i32)>().unwrap_or((0, 0, 0, 0))
            } else {
                (0, 0, 0, 0)
            }
        } else {
            (0, 0, 0, 0)
        };

        // 2. Increment counters
        match status {
            OrderStatus::New => total += 1,
            OrderStatus::Filled => filled += 1,
            OrderStatus::Cancelled => cancelled += 1,
            OrderStatus::Rejected => rejected += 1,
            OrderStatus::Expired => cancelled += 1,
             _ => {}
        }

        // 3. Upsert updated statistics
        let update_query = "
            UPDATE order_statistics SET
                total_orders = ?,
                filled_orders = ?,
                cancelled_orders = ?,
                rejected_orders = ?,
                last_order_at = ?,
                updated_at = ?
            WHERE user_id = ?
        ";

        self.session
            .query(
                update_query,
                (
                    total,
                    filled,
                    cancelled,
                    rejected,
                    timestamp as i64,
                    timestamp as i64,
                    user_id as i64,
                ),
            )
            .await?;

        Ok(())
    }

    /// Initialize statistics for a new user
    pub async fn init_user_statistics(&self, user_id: u64, timestamp: u64) -> Result<()> {
        let query = "
            INSERT INTO order_statistics (
                user_id, total_orders, filled_orders, cancelled_orders, rejected_orders,
                total_volume, last_order_at, updated_at
            ) VALUES (?, 1, 0, 0, 0, 0, ?, ?)
            IF NOT EXISTS
        ";

        self.session
            .query(query, (user_id as i64, timestamp as i64, timestamp as i64))
            .await?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "order_history_db_tests.rs"]
mod order_history_db_tests;
