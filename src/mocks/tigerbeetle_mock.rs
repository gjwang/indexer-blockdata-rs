/// Mock TigerBeetle Client for testing
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MockTbClient {
    balances: Arc<Mutex<HashMap<u128, i64>>>,
    pending_transfers: Arc<Mutex<HashMap<u128, PendingTransfer>>>,
}

#[derive(Debug, Clone)]
struct PendingTransfer {
    id: u128,
    debit_account: u128,
    credit_account: u128,
    amount: i64,
    voided: bool,
    posted: bool,
}

impl MockTbClient {
    pub fn new() -> Self {
        Self {
            balances: Arc::new(Mutex::new(HashMap::new())),
            pending_transfers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set initial balance for testing
    pub fn set_balance(&self, account_id: u128, balance: i64) {
        self.balances.lock().unwrap().insert(account_id, balance);
    }

    /// Get available balance
    pub fn get_available_balance(&self, account_id: u128) -> Result<i64> {
        let balances = self.balances.lock().unwrap();
        Ok(*balances.get(&account_id).unwrap_or(&0))
    }

    /// Create PENDING transfer
    pub fn create_pending_transfer(
        &self,
        transfer_id: u128,
        debit_account: u128,
        credit_account: u128,
        amount: i64,
    ) -> Result<()> {
        let mut balances = self.balances.lock().unwrap();

        // Check debit account has sufficient balance
        let debit_balance = *balances.get(&debit_account).unwrap_or(&0);
        if debit_balance < amount {
            anyhow::bail!("Insufficient balance");
        }

        // Lock funds (reduce available balance)
        balances.insert(debit_account, debit_balance - amount);
        drop(balances); // Release lock before acquiring next

        // Create pending transfer record
        let mut pending = self.pending_transfers.lock().unwrap();
        pending.insert(transfer_id, PendingTransfer {
            id: transfer_id,
            debit_account,
            credit_account,
            amount,
            voided: false,
            posted: false,
        });

        Ok(())
    }

    /// POST PENDING transfer (finalize)
    pub fn post_pending_transfer(&self, transfer_id: u128) -> Result<()> {
        // Get transfer info
        let (credit_account, amount) = {
            let mut pending = self.pending_transfers.lock().unwrap();
            let transfer = pending.get_mut(&transfer_id)
                .ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;

            if transfer.voided {
                anyhow::bail!("Transfer already voided");
            }
            if transfer.posted {
                anyhow::bail!("Transfer already posted");
            }

            transfer.posted = true;
            (transfer.credit_account, transfer.amount)
        };

        // Move funds to credit account
        let mut balances = self.balances.lock().unwrap();
        let credit_balance = *balances.get(&credit_account).unwrap_or(&0);
        balances.insert(credit_account, credit_balance + amount);

        Ok(())
    }

    /// VOID PENDING transfer (cancel)
    pub fn void_pending_transfer(&self, transfer_id: u128) -> Result<()> {
        // Get transfer info
        let (debit_account, amount) = {
            let mut pending = self.pending_transfers.lock().unwrap();
            let transfer = pending.get_mut(&transfer_id)
                .ok_or_else(|| anyhow::anyhow!("Transfer not found"))?;

            if transfer.posted {
                anyhow::bail!("Transfer already posted");
            }
            if transfer.voided {
                return Ok(()); // Already voided, idempotent
            }

            transfer.voided = true;
            (transfer.debit_account, transfer.amount)
        };

        // Return funds to debit account
        let mut balances = self.balances.lock().unwrap();
        let debit_balance = *balances.get(&debit_account).unwrap_or(&0);
        balances.insert(debit_account, debit_balance + amount);

        Ok(())
    }

    /// Lookup transfer status
    pub fn lookup_transfer(&self, transfer_id: u128) -> Option<TransferStatus> {
        let pending = self.pending_transfers.lock().unwrap();
        pending.get(&transfer_id).map(|t| {
            if t.posted {
                TransferStatus::Posted
            } else if t.voided {
                TransferStatus::Voided
            } else {
                TransferStatus::Pending
            }
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum TransferStatus {
    Pending,
    Posted,
    Voided,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_tb_basic_flow() {
        let client = MockTbClient::new();

        // Setup accounts
        let funding_account = 2; // user_id=0, asset_id=2
        let spot_account = ((3001_u128) << 64) | 2; // user_id=3001, asset_id=2

        // Fund the funding account
        client.set_balance(funding_account, 1000_000_000); // 10.00 USDT

        // Create pending transfer
        let transfer_id = 12345_u128;
        client.create_pending_transfer(
            transfer_id,
            funding_account,
            spot_account,
            100_000_000, // 1.00 USDT
        ).unwrap();

        // Check balances
        assert_eq!(client.get_available_balance(funding_account).unwrap(), 900_000_000);
        assert_eq!(client.get_available_balance(spot_account).unwrap(), 0);

        // Post transfer
        client.post_pending_transfer(transfer_id).unwrap();

        // Check final balances
        assert_eq!(client.get_available_balance(funding_account).unwrap(), 900_000_000);
        assert_eq!(client.get_available_balance(spot_account).unwrap(), 100_000_000);

        // Check status
        assert_eq!(client.lookup_transfer(transfer_id), Some(TransferStatus::Posted));
    }

    #[test]
    fn test_mock_tb_void() {
        let client = MockTbClient::new();

        let funding_account = 2;
        let spot_account = ((3001_u128) << 64) | 2;

        client.set_balance(funding_account, 1000_000_000);

        let transfer_id = 12345_u128;
        client.create_pending_transfer(
            transfer_id,
            funding_account,
            spot_account,
            100_000_000,
        ).unwrap();

        // Void the transfer
        client.void_pending_transfer(transfer_id).unwrap();

        // Funds should be returned
        assert_eq!(client.get_available_balance(funding_account).unwrap(), 1000_000_000);
        assert_eq!(client.get_available_balance(spot_account).unwrap(), 0);

        assert_eq!(client.lookup_transfer(transfer_id), Some(TransferStatus::Voided));
    }

    #[test]
    fn test_insufficient_balance() {
        let client = MockTbClient::new();

        let funding_account = 2;
        let spot_account = ((3001_u128) << 64) | 2;

        client.set_balance(funding_account, 50_000_000); // Only 0.50 USDT

        let transfer_id = 12345_u128;
        let result = client.create_pending_transfer(
            transfer_id,
            funding_account,
            spot_account,
            100_000_000, // Try to transfer 1.00 USDT
        );

        assert!(result.is_err());
    }
}
