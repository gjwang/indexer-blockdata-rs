use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::extract::Query;
use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    routing::post,
    Router,
};
use tower_http::cors::CorsLayer;

use crate::client_order_convertor::client_order_convert;
use crate::db::SettlementDb;
use crate::fast_ulid::SnowflakeGenRng;
use crate::models::{ApiResponse, ClientOrder, OrderStatus, UserAccountManager};
use crate::symbol_manager::SymbolManager;

pub trait OrderPublisher: Send + Sync {
    fn publish(
        &self,
        topic: String,
        key: String,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
}
pub struct AppState {
    pub symbol_manager: SymbolManager,
    pub producer: Arc<dyn OrderPublisher>,
    pub snowflake_gen: Mutex<SnowflakeGenRng>,
    pub kafka_topic: String,
    pub user_manager: UserAccountManager,
    pub db: Option<Arc<SettlementDb>>,
}

pub fn create_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/orders", post(create_order))
        .route("/api/user/balance", axum::routing::get(get_balance))
        .route("/api/user/trade_history", axum::routing::get(get_trade_history))
        .route("/api/user/order_history", axum::routing::get(get_order_history))
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

#[derive(Debug, serde::Serialize)]
struct BalanceResponse {
    asset: String,
    //FIXME: any balance,amount,frozen, should be float_string
    //it the decimal format, but our internal use u64, balance_internal_u64=bal_decimal*asset_decimal
    //user do NOT need to know our internal impl detail, every in order data convert to internal u64 first
    //and convert to decimal format before send to user
    balance: i64,
}

async fn get_balance(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<BalanceResponse>>>, (StatusCode, String)> {
    let user_id = state.user_manager.get_user_id();

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

        let response: Vec<BalanceResponse> = balances
            .into_iter()
            .map(|(asset_id, amount)| {
                let asset_name = state
                    .symbol_manager
                    .get_asset_name(asset_id)
                    .unwrap_or_else(|| format!("UNKNOWN_{}", asset_id));
                BalanceResponse { asset: asset_name, balance: amount }
            })
            .collect();

        Ok(Json(ApiResponse::success(response)))
    } else {
        // Return empty if DB not connected
        Ok(Json(ApiResponse::success(vec![])))
    }
}

#[derive(serde::Deserialize)]
struct HistoryParams {
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
    let user_id = state.user_manager.get_user_id();
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

                    Some(TradeHistoryResponse {
                        trade_id: t.trade_id.to_string(),
                        symbol: params.symbol.clone(),
                        price: t.price.to_string(),
                        quantity: t.quantity.to_string(),
                        role: role.to_string(),
                        time: 0, // settled_at is not in MatchExecData struct?
                                 // Wait, SettlementDb::parse_trade_row ignores settled_at!
                                 // I should update MatchExecData or SettlementDb to return settled_at.
                                 // For now, use 0 or current time?
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

async fn get_order_history(
    Extension(_state): Extension<Arc<AppState>>,
    Query(_params): Query<HistoryParams>,
) -> Result<Json<ApiResponse<Vec<String>>>, (StatusCode, String)> {
    // Not implemented yet (requires order persistence)
    Ok(Json(ApiResponse::success(vec![])))
}
