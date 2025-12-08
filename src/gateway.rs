use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::Query;
use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::post,
    Router,
};
use rust_decimal::Decimal;
use tokio::time::sleep;
use tower_http::cors::CorsLayer;

use crate::client_order_convertor::client_order_convert;
use crate::db::SettlementDb;
use crate::fast_ulid::SnowflakeGenRng;
use crate::ledger::MatchExecData;
use crate::models::balance_manager::{BalanceManager, ClientBalance};
use crate::models::{
    u64_to_decimal_string, ApiResponse, BalanceRequest, ClientOrder, OrderStatus,
    UserAccountManager,
};
use crate::symbol_manager::SymbolManager;
use crate::user_account::Balance;

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
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
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
    pub db: Option<SettlementDb>,
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
    let (asset_id, raw_amount) =
        state.balance_manager.to_internal_amount(&payload.asset, payload.amount).map_err(|e| {
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
    println!("⏳ Waiting 1s for settlement...");
    sleep(Duration::from_secs(1)).await;

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
    let (asset_id, raw_amount) =
        state.balance_manager.to_internal_amount(&payload.asset, payload.amount).map_err(|e| {
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

    // Update Balance in DB using append-only ledger
    if let Some(db) = &state.db {
        // Get current seq and use next seq for withdraw
        let current = db
            .get_current_balance(payload.user_id, asset_id)
            .await
            .map_err(|e| {
                eprintln!("DB Error getting balance: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or_else(|| {
                eprintln!(
                    "No balance for user {} asset {} - deposit required first",
                    payload.user_id, asset_id
                );
                StatusCode::BAD_REQUEST
            })?;

        let next_seq = (current.seq + 1) as u64;
        db.withdraw(payload.user_id, asset_id, raw_amount, next_seq, 0).await.map_err(|e| {
            eprintln!("DB Error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    } else {
        eprintln!("DB not initialized");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

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
    let (order_id, internal_order) = client_order_convert(
        &client_order,
        &state.symbol_manager,
        &state.balance_manager,
        &state.snowflake_gen,
        user_id,
    )?;

    // Send to Kafka (use bincode for internal messages - UBSCore expects bincode)
    let payload = bincode::serialize(&internal_order).unwrap();
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
        // Use the materialized view for efficient balance retrieval
        let balances = db
            .get_user_all_balances(user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut response = Vec::new();
        for balance in balances {
            if let Some(client_balance) = state.balance_manager.to_client_balance(
                balance.asset_id, // Removed incorrect 'as i32' cast
                balance.avail,
                balance.frozen,
            ) {
                response.push(client_balance);
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
struct DisplayTradeHistoryResponse {
    trade_id: String,
    symbol: String,
    price: Decimal,
    quantity: Decimal,
    role: String,
    time: u64,
}

impl DisplayTradeHistoryResponse {
    pub fn new(
        trade: &MatchExecData,
        symbol: &str,
        role: &str,
        balance_manager: &BalanceManager,
        base_asset_id: u32,
    ) -> Option<Self> {
        let price_decimal = balance_manager.to_client_price(symbol, trade.price)?;
        let qty_decimal = balance_manager.to_client_amount(base_asset_id, trade.quantity)?;

        Some(Self {
            trade_id: trade.trade_id.to_string(),
            symbol: symbol.to_string(),
            price: price_decimal,
            quantity: qty_decimal,
            role: role.to_string(),
            time: trade.settled_at,
        })
    }
}

async fn get_trade_history(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<DisplayTradeHistoryResponse>>>, (StatusCode, String)> {
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

        let response: Vec<DisplayTradeHistoryResponse> = trades
            .into_iter()
            .filter_map(|t| {
                // Filter by symbol (client side filtering as DB query is by user)
                let pair_id = state.symbol_manager.get_symbol_id(&params.symbol)?;
                let info = state.symbol_manager.get_symbol_info_by_id(pair_id)?;
                let base = info.base_asset_id;
                let quote = info.quote_asset_id;

                if t.base_asset_id == base && t.quote_asset_id == quote {
                    let role = if t.buyer_user_id == user_id { "BUYER" } else { "SELLER" };

                    DisplayTradeHistoryResponse::new(
                        &t,
                        &params.symbol,
                        role,
                        &state.balance_manager,
                        base,
                    )
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

        // Intermediate struct for aggregation
        struct OrderAgg {
            symbol: String,
            side: String,
            price: u64,
            quantity: u64,
            time: i64,
            decimals: u32,
            price_decimals: u32,
        }

        let mut orders_map: std::collections::HashMap<u64, OrderAgg> =
            std::collections::HashMap::new();

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

            if t.base_asset_id != base || t.quote_asset_id != quote {
                continue;
            }

            let price_decimals = info.price_decimal;
            let qty_decimals = state.symbol_manager.get_asset_decimal(base).unwrap_or(8);

            // Determine if user was buyer or seller
            if t.buyer_user_id == user_id {
                let entry = orders_map.entry(t.buy_order_id).or_insert(OrderAgg {
                    symbol: params.symbol.clone(),
                    side: "BUY".to_string(),
                    price: t.price,
                    quantity: 0,
                    time: t.settled_at as i64,
                    decimals: qty_decimals,
                    price_decimals,
                });
                entry.quantity += t.quantity;
                // Keep the latest time
                if (t.settled_at as i64) > entry.time {
                    entry.time = t.settled_at as i64;
                }
            }

            if t.seller_user_id == user_id {
                let entry = orders_map.entry(t.sell_order_id).or_insert(OrderAgg {
                    symbol: params.symbol.clone(),
                    side: "SELL".to_string(),
                    price: t.price,
                    quantity: 0,
                    time: t.settled_at as i64,
                    decimals: qty_decimals,
                    price_decimals,
                });
                entry.quantity += t.quantity;
                // Keep the latest time
                if (t.settled_at as i64) > entry.time {
                    entry.time = t.settled_at as i64;
                }
            }
        }

        let mut response: Vec<OrderHistoryResponse> = orders_map
            .into_iter()
            .map(|(order_id, agg)| OrderHistoryResponse {
                order_id: order_id.to_string(),
                symbol: agg.symbol,
                side: agg.side,
                price: u64_to_decimal_string(agg.price, agg.price_decimals),
                quantity: u64_to_decimal_string(agg.quantity, agg.decimals),
                status: "FILLED".to_string(),
                time: agg.time,
            })
            .collect();

        // Sort by time descending (most recent first)
        response.sort_by(|a, b| b.time.cmp(&a.time));

        Ok(Json(ApiResponse::success(response)))
    } else {
        Ok(Json(ApiResponse::success(vec![])))
    }
}
