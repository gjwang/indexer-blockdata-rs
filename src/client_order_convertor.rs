use std::sync::Mutex;

use axum::http::StatusCode;

use crate::fast_ulid::SnowflakeGenRng;
use crate::models::ClientOrder;
use crate::symbol_manager::SymbolManager;
use crate::ubs_core::InternalOrder;

use crate::models::balance_manager::BalanceManager;

pub fn client_order_convert(
    client_order: &ClientOrder,
    symbol_manager: &SymbolManager,
    balance_manager: &BalanceManager,
    snowflake_gen: &Mutex<SnowflakeGenRng>,
    user_id: u64,
) -> Result<(u64, InternalOrder), (StatusCode, String)> {
    // Validate
    client_order.validate_order().map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Generate internal order ID
    let order_id = {
        let mut gen = snowflake_gen.lock().unwrap();
        gen.generate()
    };

    // Convert to UBSCore InternalOrder
    let internal_order = client_order
        .to_ubs_internal(symbol_manager, balance_manager, order_id, user_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    Ok((order_id, internal_order))
}
