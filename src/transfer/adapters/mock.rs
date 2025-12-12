//! Mock adapter for testing
//!
//! Allows setting expected results for each operation.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

use crate::transfer::types::OpResult;
use super::traits::ServiceAdapter;

/// Mock adapter for testing
pub struct MockAdapter {
    name: String,
    /// Map of req_id -> expected result
    results: Mutex<HashMap<Uuid, OpResult>>,
    /// Default result when no specific result is set
    default_result: Mutex<OpResult>,
}

impl MockAdapter {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            results: Mutex::new(HashMap::new()),
            default_result: Mutex::new(OpResult::Success),
        }
    }

    /// Set expected result for a specific req_id
    pub fn set_result(&self, req_id: Uuid, result: OpResult) {
        self.results.lock().unwrap().insert(req_id, result);
    }

    /// Set default result for all operations
    pub fn set_default_result(&self, result: OpResult) {
        *self.default_result.lock().unwrap() = result;
    }

    /// Clear all set results
    pub fn clear(&self) {
        self.results.lock().unwrap().clear();
    }

    fn get_result(&self, req_id: Uuid) -> OpResult {
        self.results
            .lock()
            .unwrap()
            .get(&req_id)
            .cloned()
            .unwrap_or_else(|| self.default_result.lock().unwrap().clone())
    }
}

#[async_trait]
impl ServiceAdapter for MockAdapter {
    async fn withdraw(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::debug!(
            "[{}] withdraw({}, user={}, asset={}, amount={})",
            self.name, req_id, user_id, asset_id, amount
        );
        self.get_result(req_id)
    }

    async fn deposit(
        &self,
        req_id: Uuid,
        user_id: u64,
        asset_id: u32,
        amount: u64,
    ) -> OpResult {
        log::debug!(
            "[{}] deposit({}, user={}, asset={}, amount={})",
            self.name, req_id, user_id, asset_id, amount
        );
        self.get_result(req_id)
    }

    async fn commit(&self, req_id: Uuid) -> OpResult {
        log::debug!("[{}] commit({})", self.name, req_id);
        OpResult::Success
    }

    async fn rollback(&self, req_id: Uuid) -> OpResult {
        log::debug!("[{}] rollback({})", self.name, req_id);
        OpResult::Success
    }

    async fn query(&self, req_id: Uuid) -> OpResult {
        log::debug!("[{}] query({})", self.name, req_id);
        self.get_result(req_id)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_default_success() {
        let mock = MockAdapter::new("test");
        let req_id = Uuid::new_v4();

        let result = mock.withdraw(req_id, 4001, 1, 1000).await;
        assert!(matches!(result, OpResult::Success));
    }

    #[tokio::test]
    async fn test_mock_set_result() {
        let mock = MockAdapter::new("test");
        let req_id = Uuid::new_v4();

        mock.set_result(req_id, OpResult::Failed("Insufficient funds".to_string()));

        let result = mock.withdraw(req_id, 4001, 1, 1000).await;
        assert!(matches!(result, OpResult::Failed(_)));
    }

    #[tokio::test]
    async fn test_mock_set_default() {
        let mock = MockAdapter::new("test");
        mock.set_default_result(OpResult::Pending);

        let req_id = Uuid::new_v4();
        let result = mock.withdraw(req_id, 4001, 1, 1000).await;
        assert!(matches!(result, OpResult::Pending));
    }
}
