use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use rust_decimal::Decimal;
use tower::util::ServiceExt;

use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::gateway::{create_app, AppState, OrderPublisher, SimulatedFundingAccount};
use fetcher::models::balance_manager::BalanceManager;
use fetcher::models::{ClientOrder, OrderType, Side, UserAccountManager};
use fetcher::symbol_manager::SymbolManager;

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
async fn test_create_order_api_success() {
    let mut sm = SymbolManager::new();
    sm.add_asset(1, 8, 3, "BTC"); // BTC
    sm.add_asset(2, 8, 2, "USDT"); // USDT
    sm.insert("BTC_USDT", 1, 1, 2);
    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

    let state = Arc::new(AppState {
        symbol_manager: Arc::new(sm.clone()),
        balance_manager: BalanceManager::new(Arc::new(sm)),
        producer: Arc::new(MockPublisher),
        snowflake_gen,
        kafka_topic: "test_topic".to_string(),
        balance_topic: "test_balance_topic".to_string(),
        user_manager: UserAccountManager::new(),
        db: None,
        funding_account: Arc::new(Mutex::new(SimulatedFundingAccount::new())),
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
                .uri("/api/orders?user_id=1")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&client_order).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    if status != StatusCode::OK {
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        println!("Response body: {:?}", String::from_utf8_lossy(&body_bytes));
        panic!("Status not OK: {}", status);
    }
    assert_eq!(status, StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(body_json["status"], 0);
    assert_eq!(body_json["msg"], "ok");
    assert!(body_json["data"]["order_id"].is_string());
    assert_eq!(body_json["data"]["cid"], "clientid1234567890123");
}
