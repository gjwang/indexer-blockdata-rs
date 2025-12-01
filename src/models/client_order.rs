use crate::models::{OrderRequest, OrderType, Side};
use crate::symbol_manager::SymbolManager;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ClientOrder {
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub price: u64,
    pub quantity: u64,
    pub user_id: u64,
    pub order_type: String,
}

impl ClientOrder {
    /// Create a new ClientOrder with validation
    pub fn new(
        client_order_id: String,
        symbol: String,
        side: String,
        price: u64,
        quantity: u64,
        user_id: u64,
        order_type: String,
    ) -> Result<Self, String> {
        let order = ClientOrder {
            client_order_id,
            symbol,
            side,
            price,
            quantity,
            user_id,
            order_type,
        };
        order.validate()?;
        Ok(order)
    }

    /// Create ClientOrder from JSON string
    pub fn from_json(json: &str) -> Result<Self, String> {
        let order: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        order.validate()?;
        Ok(order)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.price == 0 {
            return Err("Price must be greater than 0".to_string());
        }
        if self.quantity == 0 {
            return Err("Quantity must be greater than 0".to_string());
        }
        if self.client_order_id.len() >= 32 {
            return Err("Client order ID must be less than 32 characters".to_string());
        }
        if !self.client_order_id.chars().all(char::is_alphanumeric) {
            return Err("Client order ID must be alphanumeric".to_string());
        }
        Ok(())
    }

    /// Convert ClientOrder to internal OrderRequest
    pub fn try_to_internal(
        &self,
        symbol_manager: &SymbolManager,
        order_id: u64,
    ) -> Result<OrderRequest, String> {
        self.validate()?;

        let symbol_id = symbol_manager
            .get_id(&self.symbol)
            .ok_or_else(|| format!("Unknown symbol: {}", self.symbol))?;

        let side: Side = self
            .side
            .parse()
            .map_err(|e| format!("Invalid side: {}", e))?;
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

    /// Convert internal OrderRequest back to ClientOrder
    pub fn try_from_internal(
        request: &OrderRequest,
        symbol_manager: &SymbolManager,
    ) -> Result<Self, String> {
        match request {
            OrderRequest::PlaceOrder {
                symbol_id,
                side,
                price,
                quantity,
                user_id,
                order_type,
                ..
            } => {
                let symbol = symbol_manager
                    .get_symbol(*symbol_id)
                    .ok_or_else(|| format!("Unknown symbol ID: {}", symbol_id))?
                    .clone();

                Ok(ClientOrder {
                    client_order_id: "".to_string(), // OrderRequest doesn't store client_order_id yet
                    symbol,
                    side: side.to_string(),
                    price: *price,
                    quantity: *quantity,
                    user_id: *user_id,
                    order_type: order_type.to_string(),
                })
            }
            _ => Err("Only PlaceOrder can be converted to ClientOrder".to_string()),
        }
    }
}
