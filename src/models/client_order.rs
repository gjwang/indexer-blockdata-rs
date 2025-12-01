use crate::models::{OrderRequest, OrderType, Side};
use crate::symbol_manager::SymbolManager;

pub struct ClientRawOrder {
    pub symbol: String,
    pub side: String,
    pub price: u64,
    pub quantity: u64,
    pub user_id: u64,
    pub order_type: String,
}

impl ClientRawOrder {
    pub fn to_internal(
        &self,
        symbol_manager: &SymbolManager,
        order_id: u64,
    ) -> Result<OrderRequest, String> {
        let symbol_id = symbol_manager
            .get_id(&self.symbol)
            .ok_or_else(|| format!("Unknown symbol: {}", self.symbol))?;

        let side: Side = self.side.parse().map_err(|e| format!("Invalid side: {}", e))?;
        let order_type: OrderType = self
            .order_type
            .parse()
            .map_err(|e| format!("Invalid order type: {}", e))?;

        Ok(OrderRequest::PlaceOrder {
            order_id,
            user_id: self.user_id,
            symbol_id,
            side,
            price: self.price,
            quantity: self.quantity,
            order_type,
        })
    }
}
