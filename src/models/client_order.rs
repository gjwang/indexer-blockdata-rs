use crate::models::{OrderRequest, OrderType, Side};
use crate::symbol_manager::SymbolManager;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

#[derive(Debug, Deserialize, Serialize, Validate)]
pub struct ClientOrder {
    #[validate(length(min = 20, max = 32), custom(function = "validate_alphanumeric"))]
    pub client_order_id: String,
    #[validate(length(min = 3, max = 24), custom(function = "validate_symbol_format"))]
    pub symbol: String,
    #[validate(custom(function = "validate_side"))]
    pub side: String,
    #[validate(range(min = 1))]
    pub price: u64,//TODO: min = 1, max = 10^18
    #[validate(range(min = 1))]
    pub quantity: u64,//TODO: min = 1, max = 10^18
    pub user_id: u64,//TODO: min = 1, max = 10^18
    #[validate(custom(function = "validate_order_type"))]
    pub order_type: String,//TODO: OrderType enum
}

fn validate_alphanumeric(id: &str) -> Result<(), ValidationError> {
    if id.chars().all(char::is_alphanumeric) {
        Ok(())
    } else {
        let mut err = ValidationError::new("alphanumeric");
        err.message = Some("Client order ID must be alphanumeric".into());
        Err(err)
    }
}

fn validate_symbol_format(symbol: &str) -> Result<(), ValidationError> {
    if !symbol.chars().all(|c| c.is_alphanumeric() || c == '_') {
        let mut err = ValidationError::new("invalid_symbol_chars");
        err.message = Some("Symbol must be alphanumeric (including underscore)".into());
        return Err(err);
    }
    if symbol != symbol.to_uppercase() {
        let mut err = ValidationError::new("invalid_symbol_case");
        err.message = Some("Symbol must be uppercase".into());
        return Err(err);
    }
    Ok(())
}

fn validate_side(side: &str) -> Result<(), ValidationError> {
    match side.parse::<Side>() {
        Ok(_) => Ok(()),
        Err(_) => {
            let mut err = ValidationError::new("invalid_side");
            err.message = Some("Invalid side. Must be 'Buy' or 'Sell'".into());
            Err(err)
        }
    }
}

fn validate_order_type(order_type: &str) -> Result<(), ValidationError> {
    match order_type.parse::<OrderType>() {
        Ok(_) => Ok(()),
        Err(_) => {
            let mut err = ValidationError::new("invalid_order_type");
            err.message = Some("Invalid order type. Must be 'Limit' or 'Market'".into());
            Err(err)
        }
    }
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
        order.validate_order()?;
        Ok(order)
    }

    /// Create ClientOrder from JSON string
    pub fn from_json(json: &str) -> Result<Self, String> {
        let order: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        // Validation must be triggered explicitly. Attributes alone don't enforce it.
        // serde_json::from_str only handles parsing and type conversion.
        order.validate_order()?;
        Ok(order)
    }

    pub fn validate_order(&self) -> Result<(), String> {
        Validate::validate(self).map_err(|e| e.to_string())
    }

    /// Convert ClientOrder to internal OrderRequest
    pub fn try_to_internal(
        &self,
        symbol_manager: &SymbolManager,
        order_id: u64,
    ) -> Result<OrderRequest, String> {
        self.validate_order()?;

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
