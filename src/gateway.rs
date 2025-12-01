use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::post,
    Router,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;

use crate::client_order_convertor::client_order_convert;
use crate::fast_ulid::SnowflakeGenRng;
use crate::models::{ApiResponse, ClientOrder, OrderStatus, UserAccountManager};
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
    pub user_manager: UserAccountManager,
}

pub fn create_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/orders", post(create_order))
        .layer(Extension(state))
        .layer(CorsLayer::permissive())
}

#[derive(Debug, serde::Serialize)]
struct OrderResponseData {
    order_id: String,
    order_status: OrderStatus,
    cid: Option<String>,
}

async fn create_order(
    Extension(state): Extension<Arc<AppState>>,
    Json(client_order): Json<ClientOrder>,
) -> Result<Json<ApiResponse<OrderResponseData>>, (StatusCode, String)> {
    let user_id = state.user_manager.get_user_id();
    let (order_id, internal_order) =
        client_order_convert(&client_order, &state.symbol_manager, &state.snowflake_gen, user_id)?;

    // Send to Kafka
    let payload = serde_json::to_string(&internal_order).unwrap();
    let key = order_id.to_string();

    state
        .producer
        .publish(state.kafka_topic.clone(), key, payload)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    println!(
        "Order {} accepted by user_id {}",
        order_id, user_id
    );

    let response_data = OrderResponseData {
        order_id: order_id.to_string(),
        order_status: OrderStatus::Accepted,
        cid: client_order.cid,
    };

    Ok(Json(ApiResponse::success(response_data)))
}
