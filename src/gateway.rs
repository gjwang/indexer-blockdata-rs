use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::post,
    Router,
};
use tower_http::cors::CorsLayer;

use crate::client_order_convertor::client_order_convert;
use crate::fast_ulid::SnowflakeGenRng;
use crate::models::ClientOrder;
use crate::symbol_manager::SymbolManager;

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

pub fn create_app(state: Arc<AppState>) -> Router {
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

    Ok(Json(serde_json::json!({
        "order_id": order_id.to_string(),
        "status": "accepted",
        "client_order_id": client_order.client_order_id
    })))
}
