use axum::body::Body;
use axum::http::{Request, StatusCode};
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher};
use fetcher::models::UserAccountManager;
use fetcher::symbol_manager::SymbolManager;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

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
    let state = Arc::new(AppState {
        symbol_manager: SymbolManager::new(),
        producer: Arc::new(MockPublisher),
        snowflake_gen: Mutex::new(SnowflakeGenRng::new(1)),
        kafka_topic: "orders".to_string(),
        user_manager: UserAccountManager::new(),
        db: None,
    });

    let app = create_app(state);

    let response = app
        .oneshot(Request::builder().uri("/api/user/balance").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
