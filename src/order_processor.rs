use crate::fast_ulid::SnowflakeGenRng;
use crate::models::{ClientOrder, OrderRequest};
use crate::symbol_manager::SymbolManager;
use axum::http::StatusCode;
use std::sync::Mutex;

pub fn process_order(
    client_order: &ClientOrder,
    symbol_manager: &SymbolManager,
    snowflake_gen: &Mutex<SnowflakeGenRng>,
) -> Result<(u64, OrderRequest), (StatusCode, String)> {
    // Validate
    client_order.validate_order().map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Generate internal order ID
    let order_id = {
        let mut gen = snowflake_gen.lock().unwrap();
        gen.generate()
    };

    // Convert to internal
    let internal_order = client_order.try_to_internal(symbol_manager, order_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    Ok((order_id, internal_order))
}
