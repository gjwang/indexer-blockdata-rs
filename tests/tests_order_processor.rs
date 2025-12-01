use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher};
use fetcher::models::ClientOrder;
use fetcher::order_processor::process_order;
use fetcher::symbol_manager::SymbolManager;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tower::util::ServiceExt;

#[test]
fn test_process_order_success() {
    let mut sm = SymbolManager::new();
    sm.insert("BTC_USDT", 1);
    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

    let client_order = ClientOrder {
        client_order_id: Some("clientid1234567890123".to_string()),
        symbol: "BTC_USDT".to_string(),
        side: "Buy".to_string(),
        price: 50000,
        quantity: 100,
        user_id: 1,
        order_type: "Limit".to_string(),
    };

    let result = process_order(&client_order, &sm, &snowflake_gen);
    assert!(result.is_ok());
    let (order_id, _internal_order) = result.unwrap();
    assert!(order_id > 0);
}

#[test]
fn test_process_order_invalid_symbol() {
    let sm = SymbolManager::new(); // Empty
    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

    let client_order = ClientOrder {
        client_order_id: Some("clientid1234567890123".to_string()),
        symbol: "BTC_USDT".to_string(),
        side: "Buy".to_string(),
        price: 50000,
        quantity: 100,
        user_id: 1,
        order_type: "Limit".to_string(),
    };

    let result = process_order(&client_order, &sm, &snowflake_gen);
    let err = result.unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    assert!(err.1.contains("Unknown symbol"));
}

struct MockPublisher;

impl OrderPublisher for MockPublisher {
    fn publish(&self, _topic: String, _key: String, _payload: String) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
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