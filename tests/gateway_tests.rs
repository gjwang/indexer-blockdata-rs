use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{AppState, create_app, OrderPublisher, SimulatedFundingAccount};
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
    ) -> Pin<Box<dyn Future<Output=Result<(), String>> + Send>> {
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
        funding_account: Arc::new(Mutex::new(SimulatedFundingAccount::new())),
    });

    let app = create_app(state);

    let response = app
        .oneshot(Request::builder().uri("/api/user/balance?user_id=1").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
