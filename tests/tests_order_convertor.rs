use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::util::ServiceExt;

use fetcher::client_order_convertor::client_order_convert;
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{AppState, create_app, OrderPublisher};
use fetcher::models::ClientOrder;
use fetcher::symbol_manager::SymbolManager;



struct MockPublisher;

impl OrderPublisher for MockPublisher {
    fn publish(&self, _topic: String, _key: String, _payload: String) -> Pin<Box<dyn Future<Output=Result<(), String>> + Send>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn test_create_order_api_success() {
    let mut sm = SymbolManager::new();
    sm.insert("BTC_USDT", 1);
    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

    let state = Arc::new(AppState {
        symbol_manager: sm,
        producer: Arc::new(MockPublisher),
        snowflake_gen,
        kafka_topic: "test_topic".to_string(),
    });

    let app = create_app(state);

    let client_order = ClientOrder {
        client_order_id: Some("clientid1234567890123".to_string()),
        symbol: "BTC_USDT".to_string(),
        side: "Buy".to_string(),
        price: 50000,
        quantity: 100,
        user_id: 1,
        order_type: "Limit".to_string(),
    };

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/orders")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&client_order).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}