use anyhow::{Context, Result};
use scylla::{FromRow, SerializeRow, Session};
use std::sync::Arc;

use crate::models::internal_transfer_types::{AccountType, TransferStatus};

/// Transfer request record for DB
#[derive(Debug, Clone, FromRow, SerializeRow)]
pub struct TransferRequestRecord {
    pub request_id: i64,
    pub from_account_type: String,
    pub from_user_id: Option<i64>,
    pub from_asset_id: i32,
    pub to_account_type: String,
    pub to_user_id: Option<i64>,
    pub to_asset_id: i32,
    pub amount: i64,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub pending_transfer_id: Option<i64>,
    pub posted_transfer_id: Option<i64>,
    pub processor: Option<String>,
    pub error_message: Option<String>,
}

const INSERT_TRANSFER_CQL: &str = "
    INSERT INTO balance_transfer_requests (
        request_id, from_account_type, from_user_id, from_asset_id,
        to_account_type, to_user_id, to_asset_id, amount,
        status, created_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
";

const UPDATE_STATUS_CQL: &str = "
    UPDATE balance_transfer_requests
    SET status = ?, updated_at = ?
    WHERE request_id = ?
    IF status = ?
";

const SELECT_BY_ID_CQL: &str = "
    SELECT
        request_id, from_account_type, from_user_id, from_asset_id,
        to_account_type, to_user_id, to_asset_id, amount,
        status, created_at, updated_at,
        pending_transfer_id, posted_transfer_id,
        processor, error_message
    FROM balance_transfer_requests
    WHERE request_id = ?
";

const SELECT_BY_STATUS_CQL: &str = "
    SELECT
        request_id, from_account_type, from_user_id, from_asset_id,
        to_account_type, to_user_id, to_asset_id, amount,
        status, created_at, updated_at,
        pending_transfer_id, posted_transfer_id,
        processor, error_message
    FROM balance_transfer_requests
    WHERE status = ?
    ALLOW FILTERING
    LIMIT 1000
";

/// DB operations for internal transfers
pub struct InternalTransferDb {
    session: Arc<Session>,
}

impl InternalTransferDb {
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Insert new transfer request
    pub async fn insert_transfer_request(&self, record: TransferRequestRecord) -> Result<()> {
        self.session
            .query(
                INSERT_TRANSFER_CQL,
                (
                    record.request_id,
                    record.from_account_type,
                    record.from_user_id,
                    record.from_asset_id,
                    record.to_account_type,
                    record.to_user_id,
                    record.to_asset_id,
                    record.amount,
                    record.status,
                    record.created_at,
                    record.updated_at,
                ),
            )
            .await
            .context("Failed to insert transfer request")?;
        Ok(())
    }

    /// Update transfer status with CAS (Compare-And-Swap)
    pub async fn update_transfer_status_cas(
        &self,
        request_id: i64,
        new_status: TransferStatus,
        expected_status: TransferStatus,
        updated_at: i64,
    ) -> Result<bool> {
        let result = self.session
            .query(
                UPDATE_STATUS_CQL,
                (
                    new_status.as_str(),
                    updated_at,
                    request_id,
                    expected_status.as_str(),
                ),
            )
            .await
            .context("Failed to update transfer status")?;

        // Check if CAS succeeded
        if let Some(rows) = result.rows {
            if let Some(row) = rows.first() {
                let applied: bool = row.columns[0]
                    .as_ref()
                    .and_then(|v| v.as_boolean())
                    .unwrap_or(false);
                return Ok(applied);
            }
        }

        Ok(false)
    }

    /// Update transfer status (simple version without CAS)
    pub async fn update_transfer_status(
        &self,
        request_id: i64,
        status: &str,
        error_message: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let update_sql = if error_message.is_some() {
            "UPDATE balance_transfer_requests SET status = ?, updated_at = ?, error_message = ? WHERE request_id = ?"
        } else {
            "UPDATE balance_transfer_requests SET status = ?, updated_at = ? WHERE request_id = ?"
        };

        if let Some(err_msg) = error_message {
            self.session
                .query(update_sql, (status, now, err_msg, request_id))
                .await
                .context("Failed to update transfer status with error")?;
        } else {
            self.session
                .query(update_sql, (status, now, request_id))
                .await
                .context("Failed to update transfer status")?;
        }

        Ok(())
    }

    /// Get transfer by request_id
    pub async fn get_transfer_by_id(&self, request_id: i64) -> Result<Option<TransferRequestRecord>> {
        let result = self.session
            .query(SELECT_BY_ID_CQL, (request_id,))
            .await
            .context("Failed to query transfer by ID")?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                let record: TransferRequestRecord = row
                    .into_typed()
                    .context("Failed to parse transfer record")?;
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// Get transfers by status
    pub async fn get_transfers_by_status(&self, status: &str) -> Result<Vec<TransferRequestRecord>> {
        let result = self.session
            .query(SELECT_BY_STATUS_CQL, (status,))
            .await
            .context("Failed to query transfers by status")?;

        let mut records = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                let record: TransferRequestRecord = row
                    .into_typed()
                    .context("Failed to parse transfer record")?;
                records.push(record);
            }
        }

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> TransferRequestRecord {
        TransferRequestRecord {
            request_id: 1234567890,
            from_account_type: "funding".to_string(),
            from_user_id: None,
            from_asset_id: 2,
            to_account_type: "spot".to_string(),
            to_user_id: Some(3001),
            to_asset_id: 2,
            amount: 100_000_000,
            status: "requesting".to_string(),
            created_at: 1702345678000,
            updated_at: 1702345678000,
            pending_transfer_id: None,
            posted_transfer_id: None,
            processor: None,
            error_message: None,
        }
    }

    #[test]
    fn test_record_creation() {
        let record = sample_record();
        assert_eq!(record.request_id, 1234567890);
        assert_eq!(record.from_account_type, "funding");
        assert_eq!(record.status, "requesting");
    }
}
