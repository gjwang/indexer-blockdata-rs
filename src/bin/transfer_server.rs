use axum::{extract::Extension, http::StatusCode, routing::post, Json, Router};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use fetcher::models::BalanceRequest;
use fetcher::user_account::Balance;

struct SimulatedFundingAccount {
    balances: HashMap<u32, Balance>,
}

impl SimulatedFundingAccount {
    fn new() -> Self {
        let mut balances = HashMap::new();
        // Initialize with large available balances
        balances.insert(
            1,
            Balance {
                avail: 1_000_000_000_000_000,
                frozen: 0,
                version: 0,
            },
        ); // BTC
        balances.insert(
            2,
            Balance {
                avail: 1_000_000_000_000_000,
                frozen: 0,
                version: 0,
            },
        ); // USDT
        balances.insert(
            3,
            Balance {
                avail: 1_000_000_000_000_000,
                frozen: 0,
                version: 0,
            },
        ); // ETH
        Self { balances }
    }

    /// Lock funds for Transfer In (Funding -> Trading)
    fn lock(&mut self, asset_id: u32, amount: u64) -> Result<(), String> {
        let balance = self
            .balances
            .get_mut(&asset_id)
            .ok_or_else(|| format!("Asset {} not found in funding account", asset_id))?;

        balance
            .frozen(amount)
            .map_err(|e| format!("Lock failed: {}", e))
    }

    /// Finalize Transfer In: Remove from locked (funds moved to Trading Engine)
    fn spend(&mut self, asset_id: u32, amount: u64) -> Result<(), String> {
        let balance = self
            .balances
            .get_mut(&asset_id)
            .ok_or_else(|| format!("Asset {} not found in funding account", asset_id))?;

        balance
            .spend_frozen(amount)
            .map_err(|e| format!("Spend failed: {}", e))
    }

    /// Finalize Transfer Out: Add to available (funds received from Trading Engine)
    fn credit(&mut self, asset_id: u32, amount: u64) {
        let balance = self.balances.entry(asset_id).or_insert(Balance::default());
        // We ignore error here as deposit shouldn't fail unless overflow
        let _ = balance.deposit(amount);
    }
}

#[derive(Clone)]
struct AppState {
    producer: FutureProducer,
    topic: String,
    funding_account: Arc<Mutex<SimulatedFundingAccount>>,
}

#[derive(Debug, Deserialize)]
struct TransferInRequestPayload {
    request_id: String,
    user_id: u64,
    asset_id: u32,
    amount: u64,
}

#[derive(Debug, Deserialize)]
struct TransferOutRequestPayload {
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

async fn transfer_in(
    Extension(state): Extension<AppState>,
    Json(payload): Json<TransferInRequestPayload>,
) -> Result<Json<ApiResponse>, StatusCode> {
    println!("📥 Transfer In request received: {:?}", payload);

    // 1. Lock Funding Account & Reserve Funds
    {
        let mut funding = state.funding_account.lock().unwrap();
        if let Err(e) = funding.lock(payload.asset_id, payload.amount) {
            eprintln!("❌ Lock failed: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
        println!("🔒 Funds locked in funding account (pending transfer)");
    }

    // 2. Create Request & Send to Kafka
    let balance_req = BalanceRequest::TransferIn {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
        timestamp: current_time_ms(),
    };

    let json_payload = serde_json::to_string(&balance_req).map_err(|e| {
        eprintln!("Failed to serialize transfer_in request: {}", e);
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
            eprintln!("Failed to send transfer_in to Kafka: {}", e);
            // TODO: Rollback locked funds here if Kafka fails!
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    println!(
        "✅ Transfer In request published to Kafka: {}",
        payload.request_id
    );

    // 3. Wait for Settlement (Simulated)
    println!("⏳ Waiting 5s for settlement...");
    sleep(Duration::from_secs(5)).await;

    // 4. "Spend" the locked funds
    {
        let mut funding = state.funding_account.lock().unwrap();
        if let Err(e) = funding.spend(payload.asset_id, payload.amount) {
            eprintln!("❌ Critical: Failed to spend locked funds: {}", e);
            // In production this would be a critical alert
        } else {
            println!("💰 Locked funds spent. Transfer In complete.");
        }
    }

    Ok(Json(ApiResponse {
        success: true,
        message: format!(
            "Transfer In request submitted & settled: {} units of asset {} transferred to user {}",
            payload.amount, payload.asset_id, payload.user_id
        ),
        request_id: Some(payload.request_id),
    }))
}

async fn transfer_out(
    Extension(state): Extension<AppState>,
    Json(payload): Json<TransferOutRequestPayload>,
) -> Result<Json<ApiResponse>, StatusCode> {
    println!("📤 Transfer Out request received: {:?}", payload);

    // 1. Create Request & Send to Kafka
    let balance_req = BalanceRequest::TransferOut {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id: payload.asset_id,
        amount: payload.amount,
        timestamp: current_time_ms(),
    };

    let json_payload = serde_json::to_string(&balance_req).map_err(|e| {
        eprintln!("Failed to serialize transfer_out request: {}", e);
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
            eprintln!("Failed to send transfer_out to Kafka: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    println!(
        "✅ Transfer Out request published to Kafka: {}",
        payload.request_id
    );

    // 2. Wait for Settlement (Simulated)
    println!("⏳ Waiting 5s for settlement...");
    sleep(Duration::from_secs(5)).await;

    // 3. Credit funds to funding account (Funds coming from Trading Engine)
    {
        let mut funding = state.funding_account.lock().unwrap();
        funding.credit(payload.asset_id, payload.amount);
        println!("💰 Funds credited to funding account (Transfer Out complete).");
    }

    Ok(Json(ApiResponse {
        success: true,
        message: format!(
            "Transfer Out request submitted & settled: {} units of asset {} transferred from user {} to funding account",
            payload.amount, payload.asset_id, payload.user_id
        ),
        request_id: Some(payload.request_id),
    }))
}

async fn health() -> Json<ApiResponse> {
    Json(ApiResponse {
        success: true,
        message: "Transfer Server is healthy".to_string(),
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
        funding_account: Arc::new(Mutex::new(SimulatedFundingAccount::new())),
    };

    // Build router
    let app = Router::new()
        .route("/api/v1/transfer_in", post(transfer_in))
        .route("/api/v1/transfer_out", post(transfer_out))
        .route("/health", axum::routing::get(health))
        .layer(Extension(state));

    let addr = "0.0.0.0:8083";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    println!("--------------------------------------------------");
    println!("Transfer Server Started");
    println!("  Listening on:      {}", addr);
    println!("  Kafka Broker:      {}", config.kafka.broker);
    println!("  Balance Topic:     {}", balance_topic);
    println!("--------------------------------------------------");
    println!("Endpoints:");
    println!("  POST /api/v1/transfer_in   - Lock funding -> Transfer -> Wait -> Spend");
    println!("  POST /api/v1/transfer_out  - Transfer -> Wait -> Release to funding");
    println!("  GET  /health               - Health check");
    println!("--------------------------------------------------");

    axum::serve(listener, app).await.unwrap();
}
