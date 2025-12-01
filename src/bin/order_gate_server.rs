use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::post,
    Router,
};
use fetcher::fast_ulid::SnowflakeGenRng;
use fetcher::models::ClientOrder;
use fetcher::symbol_manager::SymbolManager;
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower_http::cors::CorsLayer;

struct AppState {
    symbol_manager: SymbolManager,
    producer: FutureProducer,
    snowflake_gen: Mutex<SnowflakeGenRng>,
    kafka_topic: String,
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    // Initialize SymbolManager to map strings to IDs
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
        producer,
        snowflake_gen,
        kafka_topic: config.kafka.topic,
    });

    let app = Router::new()
        .route("/api/orders", post(create_order))
        .layer(Extension(state))
        .layer(CorsLayer::permissive());

    println!("🚀 Order Gateway API running on http://localhost:3001");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn create_order(
    Extension(state): Extension<Arc<AppState>>,
    Json(client_order): Json<ClientOrder>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (order_id, internal_order) = process_order(&client_order, &state.symbol_manager, &state.snowflake_gen)?;

    // Send to Kafka
    let payload = serde_json::to_string(&internal_order).unwrap();
    let key = order_id.to_string();
    let record = FutureRecord::to(&state.kafka_topic)
        .payload(&payload)
        .key(&key);

    match state.producer.send(record, Duration::from_secs(0)).await {
        Ok((partition, offset)) => {
            println!("Sent Order {} to user_id {}", order_id, client_order.user_id);
        }
        Err((e, _)) => {
            eprintln!("Error sending order {}: {:?}", order_id, e);
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
    }

    Ok(Json(serde_json::json!({
        "order_id": order_id.to_string(),
        "status": "accepted",
        "client_order_id": client_order.client_order_id
    })))
}

fn process_order(
    client_order: &ClientOrder,
    symbol_manager: &SymbolManager,
    snowflake_gen: &Mutex<SnowflakeGenRng>,
) -> Result<(u64, fetcher::models::OrderRequest), (StatusCode, String)> {
    // Validate
    client_order.validate_order().map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Generate internal order ID
    let order_id = {
        let mut gen = snowflake_gen.lock().unwrap();
        gen.generate()
    };

    // Convert to internal
    let internal_order = client_order.try_to_internal(symbol_manager, order_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    Ok((order_id, internal_order))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fetcher::models::ClientOrder;

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
}
