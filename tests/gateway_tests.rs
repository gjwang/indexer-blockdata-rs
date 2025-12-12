use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher, SimulatedFundingAccount};
use fetcher::models::balance_manager::BalanceManager;
use fetcher::models::UserAccountManager;
use fetcher::symbol_manager::SymbolManager;

// Mock Publisher
struct MockPublisher;
impl OrderPublisher for MockPublisher {
    fn publish(
        &self,
        _topic: String,
        _key: String,
        _payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn test_get_balance_no_db() {
    let sm = Arc::new(SymbolManager::new());
    let state = Arc::new(AppState {
        symbol_manager: sm.clone(),
        balance_manager: BalanceManager::new(sm),
        producer: Arc::new(MockPublisher),
        snowflake_gen: Mutex::new(SnowflakeGenRng::new(1)),
        kafka_topic: "orders".to_string(),
        balance_topic: "balance_ops".to_string(),
        user_manager: UserAccountManager::new(),
        db: None,
        internal_transfer_db: None,
        funding_account: Arc::new(AsyncMutex::new(SimulatedFundingAccount::new())),
        #[cfg(feature = "aeron")]
        ubs_client: {
            // This test doesn't use aeron, but the field is required
            panic!("This test should not be run with aeron feature")
        },
        ubscore_timeout_ms: 1000,
        tb_client: None,
        transfer_coordinator: None,
        transfer_worker: None,
        transfer_queue: None,
    });

    let app = create_app(state);

    let response = app
        .oneshot(Request::builder().uri("/api/v1/user/balance?user_id=1").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // Note: Without DB, the endpoint may return an error or not found
    // Just verify it doesn't panic
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
}
