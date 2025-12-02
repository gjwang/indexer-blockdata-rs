use crate::models::{OrderRequest, OrderType, Side};
use crate::symbol_manager::SymbolManager;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

#[derive(Debug, Deserialize, Serialize, Validate)]
pub struct ClientOrder {
    #[validate(length(min = 20, max = 32), custom(function = "validate_alphanumeric"))]
    pub cid: Option<String>, // Client order ID
    #[validate(length(min = 3, max = 24), custom(function = "validate_symbol_format"))]
    pub symbol: String,
    pub side: Side,
    #[validate(custom(function = "validate_price_decimal"))]
    pub price: Decimal,
    #[validate(custom(function = "validate_quantity_decimal"))]
    pub quantity: Decimal,
    pub order_type: OrderType,
}

#[derive(Debug, Deserialize, Serialize, Validate)]
pub struct ClientRawOrder {
    pub user_id: u64,
    pub cid: Option<String>, // Client order ID
    pub symbol_id: u64,
    pub side: Side,
    pub price: u64,
    pub price_decimal: u32,
    pub quantity: u64,
    pub quantity_decimal: u32,
    pub order_type: OrderType,
}

impl ClientOrder {
    /// Create a new ClientOrder with validation
    pub fn new(
        cid: Option<String>,
        symbol: String,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        order_type: OrderType,
    ) -> Result<Self, String> {
        let order = ClientOrder {
            cid,
            symbol,
            side,
            price,
            quantity,
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

    /// Validate order including symbol existence in symbol_manager
    pub fn validate_with_symbol_manager(
        &self,
        symbol_manager: &SymbolManager,
    ) -> Result<(), String> {
        // First do basic validation
        self.validate_order()?;

        // Then validate symbol exists
        if symbol_manager.get_symbol_info(&self.symbol).is_none() {
            return Err(format!("Unknown symbol: {}", self.symbol));
        }

        Ok(())
    }

    /// Convert ClientOrder to internal OrderRequest
    pub fn try_to_internal(
        &self,
        symbol_manager: &SymbolManager,
        order_id: u64,
        user_id: u64,
    ) -> Result<OrderRequest, String> {
        // Validate order including symbol existence
        self.validate_with_symbol_manager(symbol_manager)?;

        let raw_order = self.to_raw_order(symbol_manager, user_id)?;

        Ok(OrderRequest::PlaceOrder {
            order_id,
            user_id: raw_order.user_id,
            symbol_id: raw_order.symbol_id as u32,
            side: raw_order.side,
            price: raw_order.price,
            quantity: raw_order.quantity,
            order_type: raw_order.order_type,
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
                order_type,
                ..
            } => {
                let symbol = symbol_manager
                    .get_symbol(*symbol_id)
                    .ok_or_else(|| format!("Unknown symbol ID: {}", symbol_id))?
                    .clone();

                let symbol_info = symbol_manager
                    .get_symbol_info_by_id(*symbol_id)
                    .ok_or_else(|| format!("Unknown symbol ID: {}", symbol_id))?;

                // Convert price from integer to Decimal
                let price_divisor = Decimal::from(10_u64.pow(symbol_info.price_decimal));
                let price_decimal = Decimal::from(*price) / price_divisor;

                // Convert quantity from integer to Decimal
                let quantity_decimals = symbol_manager
                    .get_asset_decimal(symbol_info.base_asset_id)
                    .unwrap_or(8);
                let quantity_divisor = Decimal::from(10_u64.pow(quantity_decimals));
                let quantity_decimal = Decimal::from(*quantity) / quantity_divisor;

                Ok(ClientOrder {
                    cid: None, // OrderRequest doesn't store cid yet
                    symbol,
                    side: *side,
                    price: price_decimal,
                    quantity: quantity_decimal,
                    order_type: *order_type,
                })
            }
            _ => Err("Only PlaceOrder can be converted to ClientOrder".to_string()),
        }
    }
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
    // Check for alphanumeric chars, uppercase, and underscore placement

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
    if !symbol.contains('_') {
        let mut err = ValidationError::new("invalid_symbol_format");
        err.message = Some("Symbol must contain an underscore".into());
        return Err(err);
    }
    if symbol.starts_with('_') || symbol.ends_with('_') {
        let mut err = ValidationError::new("invalid_symbol_format");
        err.message = Some("Underscore cannot be at the start or end".into());
        return Err(err);
    }
    if symbol.contains("__") {
        let mut err = ValidationError::new("invalid_symbol_format");
        err.message = Some("Symbol cannot contain consecutive underscores".into());
        return Err(err);
    }

    // Note: Symbol existence is validated in validate_with_symbol_manager()
    // which is called from try_to_internal()
    Ok(())
}

fn validate_price_decimal(price: &Decimal) -> Result<(), ValidationError> {
    if *price > Decimal::ZERO {
        Ok(())
    } else {
        let mut err = ValidationError::new("invalid_price");
        err.message = Some("Price must be a positive number".into());
        Err(err)
    }
}

fn validate_quantity_decimal(quantity: &Decimal) -> Result<(), ValidationError> {
    if *quantity > Decimal::ZERO {
        Ok(())
    } else {
        let mut err = ValidationError::new("invalid_quantity");
        err.message = Some("Quantity must be a positive number".into());
        Err(err)
    }
}

impl ClientOrder {
    /// Convert ClientOrder to ClientRawOrder using SymbolManager for decimal precision
    pub fn to_raw_order(
        &self,
        symbol_manager: &SymbolManager,
        user_id: u64,
    ) -> Result<ClientRawOrder, String> {
        self.validate_order()?;

        let symbol_info = symbol_manager
            .get_symbol_info(&self.symbol)
            .ok_or_else(|| format!("Unknown symbol: {}", self.symbol))?;

        // Convert price to integer representation (already Decimal, no parsing needed)
        let price_multiplier = Decimal::from(10_u64.pow(symbol_info.price_decimal));
        let price = (self.price * price_multiplier)
            .round()
            .to_string()
            .parse::<u64>()
            .map_err(|_| format!("Price overflow: {}", self.price))?;

        // Convert quantity to integer representation (already Decimal, no parsing needed)
        let quantity_decimals = symbol_manager
            .get_asset_decimal(symbol_info.base_asset_id)
            .unwrap_or(8);
        let quantity_multiplier = Decimal::from(10_u64.pow(quantity_decimals));
        let quantity = (self.quantity * quantity_multiplier)
            .round()
            .to_string()
            .parse::<u64>()
            .map_err(|_| format!("Quantity overflow: {}", self.quantity))?;

        Ok(ClientRawOrder {
            user_id,
            cid: self.cid.clone(),
            symbol_id: symbol_info.id as u64,
            side: self.side,
            price,
            price_decimal: symbol_info.price_decimal,
            quantity,
            quantity_decimal: quantity_decimals,
            order_type: self.order_type,
        })
    }
}
