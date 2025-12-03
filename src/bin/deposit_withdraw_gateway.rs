use axum::{
    extract::Extension,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use fetcher::models::{BalanceRequest, DepositSource, WithdrawDestination};

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
    chain: String,
    external_tx_id: String,
    confirmations: u32,
    required_confirmations: u32,
}

#[derive(Debug, Deserialize)]
struct WithdrawRequestPayload {
    request_id: String,
    user_id: u64,
    asset_id: u32,
    amount: u64,
    chain: String,
    address: String,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    success: bool,
    message: String,
    request_id: Option<String>,
}

async fn deposit(
    Extension(state): Extension<AppState>,
    Json(payload): Json<DepositRequestPayload>,
) -> Result<Json<ApiResponse>, StatusCode> {
    println!("📥 Deposit request received: {:?}", payload);

    // Create BalanceRequest
    let balance_req = BalanceRequest::Deposit {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
        source: DepositSource::Blockchain {
            chain: payload.chain,
            required_confirmations: payload.required_confirmations,
        },
        external_tx_id: payload.external_tx_id,
        confirmations: payload.confirmations,
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
        message: "Deposit request submitted".to_string(),
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

    // Create BalanceRequest
    let balance_req = BalanceRequest::Withdraw {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
        destination: WithdrawDestination::Blockchain {
            chain: payload.chain,
            address: payload.address,
        },
        external_address: payload.request_id.clone(), // Placeholder
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
        message: "Withdrawal request submitted and funds locked".to_string(),
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
    println!("Deposit/Withdraw Gateway Started");
    println!("  Listening on:      {}", addr);
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Balance Topic:     {}", balance_topic);
    println!("--------------------------------------------------");
    println!("Endpoints:");
    println!("  POST /api/v1/deposit");
    println!("  POST /api/v1/withdraw");
    println!("  GET  /health");
    println!("--------------------------------------------------");

    axum::serve(listener, app).await.unwrap();
}
