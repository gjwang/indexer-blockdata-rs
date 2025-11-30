use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use fetcher::centrifugo_auth::{CentrifugoTokenGenerator, TokenResponse};
use fetcher::centrifugo_publisher::CentrifugoPublisher;
use fetcher::models::{BalanceUpdate, OrderUpdate, StreamMessage, UserUpdate};

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    token_generator: Arc<CentrifugoTokenGenerator>,
    publisher: Arc<CentrifugoPublisher>,
}

/// Simulated authenticated user (in production, extract from JWT middleware)
#[derive(Clone, Debug)]
struct AuthenticatedUser {
    id: String,
    email: String,
}

/// Login request
#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

/// Login response
#[derive(Serialize)]
struct LoginResponse {
    access_token: String,
    user_id: String,
    expires_in: i64,
}

#[tokio::main]
async fn main() {
    // Initialize services
    let token_generator = Arc::new(CentrifugoTokenGenerator::new(
        "my_super_secret_key_which_is_very_long_and_secure_enough_for_hs256".to_string(),
    ));

    let publisher = Arc::new(CentrifugoPublisher::new(
        "http://localhost:8000/api".to_string(),
        "your_secure_api_key_here_change_this_in_production".to_string(),
    ));

    let state = AppState {
        token_generator,
        publisher,
    };

    // Build router
    let app = Router::new()
        .route("/api/auth/login", post(login_handler))
        .route("/api/centrifugo/token", get(centrifugo_token_handler))
        .route("/api/test/publish-balance", post(test_publish_balance))
        .route("/api/test/publish-order", post(test_publish_order))
        .layer(Extension(state))
        .layer(CorsLayer::permissive());

    println!("🚀 API Server running on http://localhost:3000");
    println!("📡 Endpoints:");
    println!("   POST /api/auth/login - Login and get access token");
    println!("   GET  /api/centrifugo/token - Get Centrifugo connection token");
    println!("   POST /api/test/publish-balance - Test balance update");
    println!("   POST /api/test/publish-order - Test order update");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Login handler (simplified - in production, validate against database)
async fn login_handler(
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    // Simplified authentication - in production, validate credentials
    if payload.username.is_empty() || payload.password.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Simulate user ID (in production, fetch from database)
    let user_id = "12345";

    // Generate access token (in production, use proper JWT with expiration)
    let access_token = format!("access_token_for_{}", user_id);

    Ok(Json(LoginResponse {
        access_token,
        user_id: user_id.to_string(),
        expires_in: 3600,
    }))
}

/// Get Centrifugo connection token
/// In production, this should be protected by authentication middleware
async fn centrifugo_token_handler(
    Extension(state): Extension<AppState>,
) -> Result<Json<TokenResponse>, StatusCode> {
    // In production, extract user from validated JWT token
    let user_id = "12345"; // Simulated

    // Generate token WITH channels claim to explicitly authorize
    let token = state
        .token_generator
        .generate_token_with_default_channels(user_id, Some(3600))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(TokenResponse {
        centrifugo_token: token,
        expires_in: 3600,
    }))
}

/// Test endpoint to publish balance update
#[derive(Deserialize)]
struct PublishBalanceRequest {
    user_id: String,
    asset: String,
    available: f64,
    locked: f64,
}

async fn test_publish_balance(
    Extension(state): Extension<AppState>,
    Json(payload): Json<PublishBalanceRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let balance = BalanceUpdate {
        asset: payload.asset,
        available: payload.available,
        locked: payload.locked,
        total: payload.available + payload.locked,
    };

    let user_update = UserUpdate::Balance(balance);
    let stream_message = StreamMessage {
        ts_ms: None,
        update: user_update,
    };

    state
        .publisher
        .publish_user_update(&payload.user_id, &stream_message)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Balance update published"
    })))
}

/// Test endpoint to publish order update
#[derive(Deserialize)]
struct PublishOrderRequest {
    user_id: String,
    order_id: String,
    symbol: String,
    side: String,
    status: String,
    price: f64,
    quantity: f64,
}

async fn test_publish_order(
    Extension(state): Extension<AppState>,
    Json(payload): Json<PublishOrderRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let order = OrderUpdate {
        order_id: payload.order_id,
        symbol: payload.symbol,
        side: payload.side,
        order_type: "limit".to_string(),
        status: payload.status,
        price: payload.price,
        quantity: payload.quantity,
        filled_quantity: 0.0,
        remaining_quantity: payload.quantity,
    };

    let user_update = UserUpdate::Order(order);
    let stream_message = StreamMessage {
        ts_ms: None,
        update: user_update,
    };

    state
        .publisher
        .publish_user_update(&payload.user_id, &stream_message)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Order update published"
    })))
}
