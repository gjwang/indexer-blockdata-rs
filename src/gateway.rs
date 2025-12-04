use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    Router,
    routing::post,
};
use axum::extract::Query;
use rust_decimal::Decimal;
use tokio::time::sleep;
use tower_http::cors::CorsLayer;

use crate::client_order_convertor::client_order_convert;
use crate::db::SettlementDb;
use crate::fast_ulid::SnowflakeGenRng;
use crate::models::{
    ApiResponse, BalanceRequest, ClientOrder, OrderStatus, u64_to_decimal_string,
    UserAccountManager,
};
use crate::symbol_manager::SymbolManager;
use crate::user_account::Balance;

#[derive(Debug, serde::Serialize)]
pub struct ClientBalance {
    pub asset: String,
    pub avail: Decimal,
    pub frozen: Decimal,
}

pub struct BalanceManager {
    symbol_manager: Arc<SymbolManager>,
}

impl BalanceManager {
    pub fn new(symbol_manager: Arc<SymbolManager>) -> Self {
        Self { symbol_manager }
    }

    pub fn to_client_balance(&self, asset_id: u32, avail: u64, frozen: u64) -> Option<ClientBalance> {
        let asset_name = self.symbol_manager.get_asset_name(asset_id)?;
        let decimals = self.symbol_manager.get_asset_decimal(asset_id)?;
        let client_precision = self.symbol_manager.get_asset_precision(asset_id).unwrap_or(decimals);
        let divisor = Decimal::from(10_u64.pow(decimals));

        let avail_dec = (Decimal::from(avail) / divisor)
            .round_dp_with_strategy(client_precision, rust_decimal::RoundingStrategy::ToZero);
        let frozen_dec = (Decimal::from(frozen) / divisor)
            .round_dp_with_strategy(client_precision, rust_decimal::RoundingStrategy::ToZero);

        Some(ClientBalance {
            asset: asset_name,
            avail: avail_dec,
            frozen: frozen_dec,
        })
    }

    pub fn to_internal_amount(&self, asset_name: &str, amount: Decimal) -> Result<(u32, u64), String> {
        let asset_id = self.symbol_manager.get_asset_id(asset_name)
            .ok_or_else(|| format!("Unknown asset: {}", asset_name))?;

        let decimals = self.symbol_manager.get_asset_decimal(asset_id)
            .ok_or_else(|| format!("Decimal not found for asset: {}", asset_name))?;
        let client_precision = self.symbol_manager.get_asset_precision(asset_id).unwrap_or(decimals);

        // Validate precision
        if amount.normalize().scale() > client_precision {
            return Err(format!("Amount {} exceeds max precision {}", amount, client_precision));
        }

        let multiplier = Decimal::from(10_u64.pow(decimals));

        let raw_amount = (amount * multiplier)
            .round()
            .to_string()
            .parse::<u64>()
            .map_err(|_| "Amount overflow".to_string())?;

        Ok((asset_id, raw_amount))
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct TransferInRequestPayload {
    pub request_id: String,
    pub user_id: u64,
    pub asset: String,
    pub amount: Decimal,
}

#[derive(Debug, serde::Deserialize)]
pub struct TransferOutRequestPayload {
    pub request_id: String,
    pub user_id: u64,
    pub asset: String,
    pub amount: Decimal,
}

#[derive(Debug, serde::Serialize)]
pub struct TransferResponse {
    pub success: bool,
    pub message: String,
    pub request_id: Option<String>,
}





#[derive(serde::Deserialize)]
struct UserIdParams {
    user_id: u64,
}


pub trait OrderPublisher: Send + Sync {
    fn publish(
        &self,
        topic: String,
        key: String,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output=Result<(), String>> + Send>>;
}

pub struct SimulatedFundingAccount {
    balances: HashMap<u32, Balance>,
}

impl SimulatedFundingAccount {
    pub fn new() -> Self {
        let mut balances = HashMap::new();
        // Initialize with large available balances
        balances.insert(1, Balance { avail: 1_000_000_000_000_000, frozen: 0, version: 0 }); // BTC
        balances.insert(2, Balance { avail: 1_000_000_000_000_000, frozen: 0, version: 0 }); // USDT
        balances.insert(3, Balance { avail: 1_000_000_000_000_000, frozen: 0, version: 0 }); // ETH
        Self { balances }
    }

    /// Lock funds for Transfer In (Funding -> Trading)
    fn lock(&mut self, asset_id: u32, amount: u64) -> Result<(), String> {
        let balance = self
            .balances
            .get_mut(&asset_id)
            .ok_or_else(|| format!("Asset {} not found in funding account", asset_id))?;

        balance.frozen(amount).map_err(|e| format!("Lock failed: {}", e))
    }

    /// Finalize Transfer In: Remove from locked (funds moved to Trading Engine)
    fn spend(&mut self, asset_id: u32, amount: u64) -> Result<(), String> {
        let balance = self
            .balances
            .get_mut(&asset_id)
            .ok_or_else(|| format!("Asset {} not found in funding account", asset_id))?;

        balance.spend_frozen(amount).map_err(|e| format!("Spend failed: {}", e))
    }

    /// Finalize Transfer Out: Add to available (funds received from Trading Engine)
    fn credit(&mut self, asset_id: u32, amount: u64) {
        let balance = self.balances.entry(asset_id).or_insert(Balance::default());
        // We ignore error here as deposit shouldn't fail unless overflow
        let _ = balance.deposit(amount);
    }
}


pub struct AppState {
    pub symbol_manager: Arc<SymbolManager>,
    pub balance_manager: BalanceManager,
    pub producer: Arc<dyn OrderPublisher>,
    pub snowflake_gen: Mutex<SnowflakeGenRng>,
    pub kafka_topic: String,
    pub balance_topic: String,
    pub user_manager: UserAccountManager,
    pub db: Option<Arc<SettlementDb>>,
    pub funding_account: Arc<Mutex<SimulatedFundingAccount>>,
}

pub fn create_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/orders", post(create_order))
        .route("/api/user/balance", axum::routing::get(get_balance))
        .route("/api/user/trade_history", axum::routing::get(get_trade_history))
        .route("/api/user/order_history", axum::routing::get(get_order_history))
        .route("/api/v1/transfer_in", post(transfer_in))
        .route("/api/v1/transfer_out", post(transfer_out))
        .layer(Extension(state))
        .layer(CorsLayer::permissive())
}

/// Get current timestamp in milliseconds
fn current_time_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

async fn transfer_in(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<TransferInRequestPayload>,
) -> Result<Json<TransferResponse>, StatusCode> {
    println!("📥 Transfer In request received: {:?}", payload);

    // Resolve Asset ID and Decimals
    let (asset_id, raw_amount) = state.balance_manager
        .to_internal_amount(&payload.asset, payload.amount)
        .map_err(|e| {
            eprintln!("Conversion failed: {}", e);
            StatusCode::BAD_REQUEST
        })?;

    // 1. Lock Funding Account & Reserve Funds
    {
        let mut funding = state.funding_account.lock().unwrap();
        if let Err(e) = funding.lock(asset_id, raw_amount) {
            eprintln!("❌ Lock failed: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
        println!("🔒 Funds locked in funding account (pending transfer)");
    }

    // 2. Create Request & Send to Kafka
    let balance_req = BalanceRequest::TransferIn {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id,
        amount: raw_amount,
        timestamp: current_time_ms(),
    };

    let json_payload = serde_json::to_string(&balance_req).map_err(|e| {
        eprintln!("Failed to serialize transfer_in request: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let key = payload.user_id.to_string();

    state
        .producer
        .publish(state.balance_topic.clone(), key, json_payload.into_bytes())
        .await
        .map_err(|e| {
            eprintln!("Failed to send transfer_in to Kafka: {}", e);
            // TODO: Rollback locked funds here if Kafka fails!
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    println!("✅ Transfer In request published to Kafka: {}", payload.request_id);

    // 3. Wait for Settlement (Simulated)
    println!("⏳ Waiting 5s for settlement...");
    sleep(Duration::from_secs(5)).await;

    // 4. "Spend" the locked funds
    {
        let mut funding = state.funding_account.lock().unwrap();
        if let Err(e) = funding.spend(asset_id, raw_amount) {
            eprintln!("❌ Critical: Failed to spend locked funds: {}", e);
            // In production this would be a critical alert
        } else {
            println!("💰 Locked funds spent. Transfer In complete.");
        }
    }

    Ok(Json(TransferResponse {
        success: true,
        message: format!(
            "Transfer In request submitted & settled: {} units of asset {} transferred to user {}",
            payload.amount, payload.asset, payload.user_id
        ),
        request_id: Some(payload.request_id),
    }))
}

async fn transfer_out(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<TransferOutRequestPayload>,
) -> Result<Json<TransferResponse>, StatusCode> {
    println!("📤 Transfer Out request received: {:?}", payload);

    // Resolve Asset ID and Decimals
    let (asset_id, raw_amount) = state.balance_manager
        .to_internal_amount(&payload.asset, payload.amount)
        .map_err(|e| {
            eprintln!("Conversion failed: {}", e);
            StatusCode::BAD_REQUEST
        })?;

    // 1. Create Request & Send to Kafka
    let balance_req = BalanceRequest::TransferOut {
        request_id: payload.request_id.clone(),
        user_id: payload.user_id,
        asset_id,
        amount: raw_amount,
        timestamp: current_time_ms(),
    };

    let json_payload = serde_json::to_string(&balance_req).map_err(|e| {
        eprintln!("Failed to serialize transfer_out request: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let key = payload.user_id.to_string();

    state
        .producer
        .publish(state.balance_topic.clone(), key, json_payload.into_bytes())
        .await
        .map_err(|e| {
            eprintln!("Failed to send transfer_out to Kafka: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    println!("✅ Transfer Out request published to Kafka: {}", payload.request_id);

    // 2. Wait for Settlement (Simulated)
    println!("⏳ Waiting 5s for settlement...");
    sleep(Duration::from_secs(5)).await;

    // 3. Credit funds to funding account (Funds coming from Trading Engine)
    {
        let mut funding = state.funding_account.lock().unwrap();
        funding.credit(asset_id, raw_amount);
        println!("💰 Funds credited to funding account (Transfer Out complete).");
    }

    Ok(Json(TransferResponse {
        success: true,
        message: format!(
            "Transfer Out request submitted & settled: {} units of asset {} transferred from user {} to funding account",
            payload.amount, payload.asset, payload.user_id
        ),
        request_id: Some(payload.request_id),
    }))
}

#[derive(Debug, serde::Serialize)]
struct OrderResponseData {
    order_id: String,
    order_status: OrderStatus,
    cid: Option<String>,
}

#[derive(serde::Deserialize)]
struct CreateOrderParams {
    user_id: u64,
}

async fn create_order(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<CreateOrderParams>,
    Json(client_order): Json<ClientOrder>,
) -> Result<Json<ApiResponse<OrderResponseData>>, (StatusCode, String)> {
    let user_id = params.user_id;
    let (order_id, internal_order) =
        client_order_convert(&client_order, &state.symbol_manager, &state.snowflake_gen, user_id)?;

    // Send to Kafka
    let payload = serde_json::to_vec(&internal_order).unwrap();
    let key = order_id.to_string();

    state
        .producer
        .publish(state.kafka_topic.clone(), key, payload)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    println!(
        "Order {} accepted by user_id {}, client_order: {:?}",
        order_id, user_id, client_order
    );

    let response_data = OrderResponseData {
        order_id: order_id.to_string(),
        order_status: OrderStatus::Accepted,
        cid: client_order.cid,
    };

    Ok(Json(ApiResponse::success(response_data)))
}


async fn get_balance(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<UserIdParams>,
) -> Result<Json<ApiResponse<Vec<ClientBalance>>>, (StatusCode, String)> {
    let user_id = params.user_id;

    if let Some(db) = &state.db {
        let events = db
            .get_ledger_events_by_user(user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut balances = std::collections::HashMap::new();
        for event in events {
            let entry = balances.entry(event.currency).or_insert(0i64);
            match event.event_type.as_str() {
                "DEPOSIT" | "TRADE_RECEIVE" => *entry += event.amount as i64,
                "WITHDRAWAL" | "TRADE_SPEND" => *entry -= event.amount as i64,
                _ => {}
            }
        }

        let mut response = Vec::new();
        for (asset_id, amount) in balances {
            let avail = if amount < 0 { 0 } else { amount as u64 };

            if let Some(client_balance) = state.balance_manager.to_client_balance(asset_id, avail, 0) {
                response.push(client_balance);
            } else {
                 return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Asset info not found for ID {}", asset_id),
                ));
            }
        }

        Ok(Json(ApiResponse::success(response)))
    } else {
        // Return empty if DB not connected
        Ok(Json(ApiResponse::success(vec![])))
    }
}

#[derive(serde::Deserialize)]
struct HistoryParams {
    user_id: u64,
    symbol: String,
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
struct TradeHistoryResponse {
    trade_id: String,
    symbol: String,
    price: String,
    quantity: String,
    role: String,
    time: i64,
}

async fn get_trade_history(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<TradeHistoryResponse>>>, (StatusCode, String)> {
    let user_id = params.user_id;
    let limit = params.limit.unwrap_or(100);

    // Validate symbol
    if state.symbol_manager.get_symbol_id(&params.symbol).is_none() {
        return Ok(Json(ApiResponse::success(vec![])));
    }

    if let Some(db) = &state.db {
        let trades = db
            .get_trades_by_user(user_id, limit as i32)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let response: Vec<TradeHistoryResponse> = trades
            .into_iter()
            .filter_map(|t| {
                // Filter by symbol (client side filtering as DB query is by user)
                let pair_id = state.symbol_manager.get_symbol_id(&params.symbol)?;
                let info = state.symbol_manager.get_symbol_info_by_id(pair_id)?;
                let base = info.base_asset_id;
                let quote = info.quote_asset_id;

                if t.base_asset == base && t.quote_asset == quote {
                    let role = if t.buyer_user_id == user_id { "BUYER" } else { "SELLER" };

                    let price_decimals = info.price_decimal;
                    let qty_decimals = state.symbol_manager.get_asset_decimal(base).unwrap_or(8);

                    Some(TradeHistoryResponse {
                        trade_id: t.trade_id.to_string(),
                        symbol: params.symbol.clone(),
                        price: u64_to_decimal_string(t.price, price_decimals),
                        quantity: u64_to_decimal_string(t.quantity, qty_decimals),
                        role: role.to_string(),
                        time: t.settled_at as i64,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(Json(ApiResponse::success(response)))
    } else {
        Ok(Json(ApiResponse::success(vec![])))
    }
}

#[derive(Debug, serde::Serialize)]
struct OrderHistoryResponse {
    order_id: String,
    symbol: String,
    side: String,
    price: String,
    quantity: String,
    status: String,
    time: i64,
}

async fn get_order_history(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<OrderHistoryResponse>>>, (StatusCode, String)> {
    let user_id = params.user_id;
    let limit = params.limit.unwrap_or(100);

    // Validate symbol
    if state.symbol_manager.get_symbol_id(&params.symbol).is_none() {
        return Ok(Json(ApiResponse::success(vec![])));
    }

    if let Some(db) = &state.db {
        let trades = db
            .get_trades_by_user(user_id, limit as i32)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // Extract unique orders from trades
        let mut orders_map = std::collections::HashMap::new();

        for t in trades {
            // Filter by symbol
            let pair_id = match state.symbol_manager.get_symbol_id(&params.symbol) {
                Some(id) => id,
                None => continue,
            };
            let info = match state.symbol_manager.get_symbol_info_by_id(pair_id) {
                Some(i) => i,
                None => continue,
            };
            let base = info.base_asset_id;
            let quote = info.quote_asset_id;

            if t.base_asset != base || t.quote_asset != quote {
                continue;
            }

            let price_decimals = info.price_decimal;
            let qty_decimals = state.symbol_manager.get_asset_decimal(base).unwrap_or(8);

            // Determine if user was buyer or seller
            if t.buyer_user_id == user_id {
                // User was buyer
                let order_id = t.buy_order_id;
                let entry = orders_map.entry(order_id).or_insert_with(|| OrderHistoryResponse {
                    order_id: order_id.to_string(),
                    symbol: params.symbol.clone(),
                    side: "BUY".to_string(),
                    price: u64_to_decimal_string(t.price, price_decimals),
                    quantity: u64_to_decimal_string(t.quantity, qty_decimals),
                    status: "FILLED".to_string(),
                    time: t.settled_at as i64,
                });
                // Accumulate quantity if order appears in multiple trades
                let current_qty: u64 = entry.quantity.replace(".", "").parse().unwrap_or(0);
                entry.quantity = u64_to_decimal_string(current_qty + t.quantity, qty_decimals);
            }

            if t.seller_user_id == user_id {
                // User was seller
                let order_id = t.sell_order_id;
                let entry = orders_map.entry(order_id).or_insert_with(|| OrderHistoryResponse {
                    order_id: order_id.to_string(),
                    symbol: params.symbol.clone(),
                    side: "SELL".to_string(),
                    price: u64_to_decimal_string(t.price, price_decimals),
                    quantity: u64_to_decimal_string(t.quantity, qty_decimals),
                    status: "FILLED".to_string(),
                    time: t.settled_at as i64,
                });
                // Accumulate quantity if order appears in multiple trades
                let current_qty: u64 = entry.quantity.replace(".", "").parse().unwrap_or(0);
                entry.quantity = u64_to_decimal_string(current_qty + t.quantity, qty_decimals);
            }
        }

        let mut response: Vec<OrderHistoryResponse> = orders_map.into_values().collect();
        // Sort by time descending (most recent first)
        response.sort_by(|a, b| b.time.cmp(&a.time));

        Ok(Json(ApiResponse::success(response)))
    } else {
        Ok(Json(ApiResponse::success(vec![])))
    }
}
