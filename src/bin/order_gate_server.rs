use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::post,
    Router,
};
use fetcher::client_order_convertor::client_order_convert;
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::models::ClientOrder;
use fetcher::symbol_manager::SymbolManager;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;

pub trait OrderPublisher: Send + Sync {
    fn publish(
        &self,
        topic: String,
        key: String,
        payload: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
}

pub struct AppState {
    pub symbol_manager: SymbolManager,
    pub producer: Arc<dyn OrderPublisher>,
    pub snowflake_gen: Mutex<SnowflakeGenRng>,
    pub kafka_topic: String,
}

struct KafkaPublisher(FutureProducer);

impl OrderPublisher for KafkaPublisher {
    fn publish(
        &self,
        topic: String,
        key: String,
        payload: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let producer = self.0.clone();
        Box::pin(async move {
            let record = FutureRecord::to(&topic).payload(&payload).key(&key);
            producer
                .send(record, Duration::from_secs(0))
                .await
                .map(|_| ())
                .map_err(|(e, _)| e.to_string())
        })
    }
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    let symbol_manager = SymbolManager::load_from_db();

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .set("linger.ms", &config.kafka.linger_ms)
        .set(
            "socket.keepalive.enable",
            &config.kafka.socket_keepalive_enable,
        )
        .create()
        .expect("Producer creation error");

    let snowflake_gen = Mutex::new(SnowflakeGenRng::new(1));

    let state = Arc::new(AppState {
        symbol_manager,
        producer: Arc::new(KafkaPublisher(producer)),
        snowflake_gen,
        kafka_topic: config.kafka.topic,
    });

    let app = create_app(state);

    println!("🚀 Order Gateway API running on http://localhost:3001");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn create_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/orders", post(create_order))
        .layer(Extension(state))
        .layer(CorsLayer::permissive())
}

async fn create_order(
    Extension(state): Extension<Arc<AppState>>,
    Json(client_order): Json<ClientOrder>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (order_id, internal_order) =
        client_order_convert(&client_order, &state.symbol_manager, &state.snowflake_gen)?;

    // Send to Kafka
    let payload = serde_json::to_string(&internal_order).unwrap();
    let key = order_id.to_string();

    state
        .producer
        .publish(state.kafka_topic.clone(), key, payload)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    println!("Order {} accepted by user_id {}", order_id, client_order.user_id);

    Ok(Json(serde_json::json!({
        "order_id": order_id.to_string(),
        "status": "accepted",
        "client_order_id": client_order.client_order_id
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
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
}