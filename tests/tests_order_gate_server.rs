use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher};

use fetcher::models::{ClientOrder, OrderType, Side, UserAccountManager};
use fetcher::symbol_manager::SymbolManager;
use rust_decimal::Decimal;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tower::util::ServiceExt;

struct MockPublisher;

impl OrderPublisher for MockPublisher {
    fn publish(
        &self,
        _topic: String,
        _key: String,
        _payload: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
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
        user_manager: UserAccountManager::new(),
    });

    let app = create_app(state);

    let client_order = ClientOrder {
        cid: Some("clientid1234567890123".to_string()),
        symbol: "BTC_USDT".to_string(),
        side: Side::Buy,
        price: Decimal::from_str("49999.99").unwrap(),
        quantity: Decimal::from_str("0.5").unwrap(),
        order_type: OrderType::Limit,
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

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(body_json["status"], 0);
    assert_eq!(body_json["msg"], "ok");
    assert!(body_json["data"]["order_id"].is_string());
    assert_eq!(body_json["data"]["cid"], "clientid1234567890123");
}
