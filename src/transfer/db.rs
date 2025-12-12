//! Database operations for internal transfers
//!
//! Uses ScyllaDB with LWT (Lightweight Transactions) for conditional updates.

use anyhow::{Context, Result};
use scylla::Session;
use std::sync::Arc;
use uuid::Uuid;

use crate::transfer::state::TransferState;
use crate::transfer::types::{ServiceId, TransferRecord};

// CQL Statements
const INSERT_TRANSFER_CQL: &str = "
    INSERT INTO trading.transfers (
        req_id, source, target, user_id, asset_id, amount,
        state, created_at, updated_at, error, retry_count
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
";

const GET_TRANSFER_BY_ID_CQL: &str = "
    SELECT req_id, source, target, user_id, asset_id, amount,
           state, created_at, updated_at, error, retry_count
    FROM trading.transfers
    WHERE req_id = ?
";

const UPDATE_STATE_IF_CQL: &str = "
    UPDATE trading.transfers
    SET state = ?, updated_at = ?
    WHERE req_id = ?
    IF state = ?
";

const UPDATE_STATE_WITH_ERROR_CQL: &str = "
    UPDATE trading.transfers
    SET state = ?, updated_at = ?, error = ?
    WHERE req_id = ?
    IF state = ?
";

const INCREMENT_RETRY_CQL: &str = "
    UPDATE trading.transfers
    SET retry_count = retry_count + 1, updated_at = ?
    WHERE req_id = ?
";

const FIND_STALE_CQL: &str = "
    SELECT req_id, source, target, user_id, asset_id, amount,
           state, created_at, updated_at, error, retry_count
    FROM trading.transfers
    WHERE state IN ('init', 'source_pending', 'source_done', 'target_pending', 'compensating')
      AND updated_at < ?
    ALLOW FILTERING
";

/// Database operations for transfers
pub struct TransferDb {
    session: Arc<Session>,
}

impl TransferDb {
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Create a new transfer record
    pub async fn create(&self, record: &TransferRecord) -> Result<()> {
        self.session
            .query(
                INSERT_TRANSFER_CQL,
                (
                    record.req_id,
                    record.source.as_str(),
                    record.target.as_str(),
                    record.user_id as i64,
                    record.asset_id as i32,
                    record.amount as i64,
                    record.state.as_str(),
                    record.created_at,
                    record.updated_at,
                    &record.error,
                    record.retry_count as i32,
                ),
            )
            .await
            .context("Failed to insert transfer record")?;
        Ok(())
    }

    /// Get transfer by req_id
    pub async fn get(&self, req_id: Uuid) -> Result<Option<TransferRecord>> {
        let result = self
            .session
            .query(GET_TRANSFER_BY_ID_CQL, (req_id,))
            .await
            .context("Failed to query transfer by ID")?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                let (
                    req_id,
                    source,
                    target,
                    user_id,
                    asset_id,
                    amount,
                    state,
                    created_at,
                    updated_at,
                    error,
                    retry_count,
                ): (Uuid, String, String, i64, i32, i64, String, i64, i64, Option<String>, i32) =
                    row.into_typed().context("Failed to parse transfer record")?;

                let record = TransferRecord {
                    req_id,
                    source: ServiceId::from_str(&source).unwrap_or(ServiceId::Funding),
                    target: ServiceId::from_str(&target).unwrap_or(ServiceId::Trading),
                    user_id: user_id as u64,
                    asset_id: asset_id as u32,
                    amount: amount as u64,
                    state: TransferState::from_str(&state).unwrap_or(TransferState::Init),
                    created_at,
                    updated_at,
                    error,
                    retry_count: retry_count as u32,
                };
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// Conditional state update (returns true if applied)
    /// CRITICAL: Uses LWT for safe concurrent updates
    pub async fn update_state_if(
        &self,
        req_id: Uuid,
        expected: TransferState,
        new_state: TransferState,
    ) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();

        let result = self
            .session
            .query(
                UPDATE_STATE_IF_CQL,
                (new_state.as_str(), now, req_id, expected.as_str()),
            )
            .await
            .context("Failed to update transfer state")?;

        // Check [applied] column from LWT result
        // IMPORTANT: Log warning if parsing fails (expert review recommendation)
        let applied = match result.first_row_typed::<(bool,)>() {
            Ok(row) => row.0,
            Err(e) => {
                log::warn!(
                    "Failed to parse LWT result for {}: {} (treating as not applied)",
                    req_id,
                    e
                );
                false
            }
        };

        Ok(applied)
    }

    /// Update state with error message
    pub async fn update_state_with_error(
        &self,
        req_id: Uuid,
        expected: TransferState,
        new_state: TransferState,
        error: &str,
    ) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();

        let result = self
            .session
            .query(
                UPDATE_STATE_WITH_ERROR_CQL,
                (new_state.as_str(), now, error, req_id, expected.as_str()),
            )
            .await
            .context("Failed to update transfer state with error")?;

        let applied = match result.first_row_typed::<(bool,)>() {
            Ok(row) => row.0,
            Err(e) => {
                log::warn!(
                    "Failed to parse LWT result for {}: {} (treating as not applied)",
                    req_id,
                    e
                );
                false
            }
        };

        Ok(applied)
    }

    /// Increment retry count and update timestamp
    pub async fn increment_retry(&self, req_id: Uuid) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();

        self.session
            .query(INCREMENT_RETRY_CQL, (now, req_id))
            .await
            .context("Failed to increment retry count")?;

        Ok(())
    }

    /// Find stale transfers (for scanner)
    /// Returns transfers not in terminal state and updated_at < cutoff
    pub async fn find_stale(&self, stale_after_ms: i64) -> Result<Vec<TransferRecord>> {
        let cutoff = chrono::Utc::now().timestamp_millis() - stale_after_ms;

        let result = self
            .session
            .query(FIND_STALE_CQL, (cutoff,))
            .await
            .context("Failed to query stale transfers")?;

        let mut records = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                let (
                    req_id,
                    source,
                    target,
                    user_id,
                    asset_id,
                    amount,
                    state,
                    created_at,
                    updated_at,
                    error,
                    retry_count,
                ): (Uuid, String, String, i64, i32, i64, String, i64, i64, Option<String>, i32) =
                    row.into_typed().context("Failed to parse stale transfer record")?;

                records.push(TransferRecord {
                    req_id,
                    source: ServiceId::from_str(&source).unwrap_or(ServiceId::Funding),
                    target: ServiceId::from_str(&target).unwrap_or(ServiceId::Trading),
                    user_id: user_id as u64,
                    asset_id: asset_id as u32,
                    amount: amount as u64,
                    state: TransferState::from_str(&state).unwrap_or(TransferState::Init),
                    created_at,
                    updated_at,
                    error,
                    retry_count: retry_count as u32,
                });
            }
        }

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cql_statements_not_empty() {
        assert!(!INSERT_TRANSFER_CQL.is_empty());
        assert!(!GET_TRANSFER_BY_ID_CQL.is_empty());
        assert!(!UPDATE_STATE_IF_CQL.is_empty());
        assert!(!UPDATE_STATE_WITH_ERROR_CQL.is_empty());
        assert!(!INCREMENT_RETRY_CQL.is_empty());
        assert!(!FIND_STALE_CQL.is_empty());
    }
}
