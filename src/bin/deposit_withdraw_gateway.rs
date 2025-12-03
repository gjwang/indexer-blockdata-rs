use axum::{
    extract::Extension,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fetcher::models::BalanceRequest;

#[derive(Clone)]
struct AppState {
    producer: FutureProducer,
    topic: String,
}

#[derive(Debug, Deserialize)]
struct DepositRequestPayload {
    request_id: String,
    user_id: u64,
    asset_id: u32,
    amount: u64,
}

#[derive(Debug, Deserialize)]
struct WithdrawRequestPayload {
    request_id: String,
    user_id: u64,
    asset_id: u32,
    amount: u64,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    success: bool,
    message: String,
    request_id: Option<String>,
}

/// Get current timestamp in milliseconds
fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn deposit(
    Extension(state): Extension<AppState>,
    Json(payload): Json<DepositRequestPayload>,
) -> Result<Json<ApiResponse>, StatusCode> {
    println!("📥 Deposit request received: {:?}", payload);

    // Create BalanceRequest with current timestamp
    let balance_req = BalanceRequest::Deposit {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
        timestamp: current_time_ms(),
    };

    // Serialize and publish to Kafka
    let json_payload = serde_json::to_string(&balance_req)
        .map_err(|e| {
            eprintln!("Failed to serialize deposit request: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let key = payload.user_id.to_string();

    state
        .producer
        .send(
            FutureRecord::to(&state.topic)
                .payload(&json_payload)
                .key(&key),
            Duration::from_secs(0),
        )
        .await
        .map_err(|(e, _)| {
            eprintln!("Failed to send deposit to Kafka: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    println!("✅ Deposit request published to Kafka: {}", payload.request_id);

    Ok(Json(ApiResponse {
        success: true,
        message: format!(
            "Deposit request submitted: {} units of asset {} will be transferred to user {}",
            payload.amount, payload.asset_id, payload.user_id
        ),
        request_id: Some(payload.request_id),
    }))
}

async fn withdraw(
    Extension(state): Extension<AppState>,
    Json(payload): Json<WithdrawRequestPayload>,
) -> Result<Json<ApiResponse>, StatusCode> {
    println!("📤 Withdraw request received: {:?}", payload);

    // TODO: Add authentication/authorization checks here
    // TODO: Add 2FA validation
    // TODO: Check withdrawal limits

    // Create BalanceRequest with current timestamp
    let balance_req = BalanceRequest::Withdraw {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
        timestamp: current_time_ms(),
    };

    // Serialize and publish to Kafka
    let json_payload = serde_json::to_string(&balance_req)
        .map_err(|e| {
            eprintln!("Failed to serialize withdraw request: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let key = payload.user_id.to_string();

    state
        .producer
        .send(
            FutureRecord::to(&state.topic)
                .payload(&json_payload)
                .key(&key),
            Duration::from_secs(0),
        )
        .await
        .map_err(|(e, _)| {
            eprintln!("Failed to send withdraw to Kafka: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    println!("✅ Withdraw request published to Kafka: {}", payload.request_id);

    Ok(Json(ApiResponse {
        success: true,
        message: format!(
            "Withdrawal request submitted: {} units of asset {} will be transferred from user {} to funding account",
            payload.amount, payload.asset_id, payload.user_id
        ),
        request_id: Some(payload.request_id),
    }))
}

async fn health() -> Json<ApiResponse> {
    Json(ApiResponse {
        success: true,
        message: "Deposit/Withdraw Gateway is healthy".to_string(),
        request_id: None,
    })
}

#[tokio::main]
async fn main() {
    let config = fetcher::configure::load_config().expect("Failed to load config");

    // Kafka Producer Setup
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &config.kafka.broker)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("Producer creation failed");

    let balance_topic = config
        .kafka
        .topics
        .balance_ops
        .as_ref()
        .unwrap_or(&"balance_ops".to_string())
        .clone();

    let state = AppState {
        producer,
        topic: balance_topic.clone(),
    };

    // Build router
    let app = Router::new()
        .route("/api/v1/deposit", post(deposit))
        .route("/api/v1/withdraw", post(withdraw))
        .route("/health", axum::routing::get(health))
        .layer(Extension(state));

    let addr = "0.0.0.0:8082";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    println!("--------------------------------------------------");
    println!("Deposit/Withdraw Gateway Started (Simplified)");
    println!("  Listening on:      {}", addr);
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Balance Topic:     {}", balance_topic);
    println!("  Time Window:       60 seconds");
    println!("--------------------------------------------------");
    println!("Endpoints:");
    println!("  POST /api/v1/deposit   - Transfer from funding_account to user");
    println!("  POST /api/v1/withdraw  - Transfer from user to funding_account");
    println!("  GET  /health           - Health check");
    println!("--------------------------------------------------");
    println!("Note: All transfers are internal (funding_account <-> trading_account)");
    println!("      Requests older than 60s will be rejected");
    println!("      Duplicate requests within 60s will be ignored");
    println!("--------------------------------------------------");

    axum::serve(listener, app).await.unwrap();
}
