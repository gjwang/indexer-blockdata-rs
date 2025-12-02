use rustc_hash::FxHashMap;

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub symbol: String,
    pub id: u32,
    pub price_decimal: u32,
    pub quantity_decimal: u32,
}

#[derive(Debug, Clone)]
pub struct AssetInfo {
    pub id: u32,
    pub symbol: String,
    pub decimals: u32,
}

/// Manages symbol-to-ID and ID-to-symbol mappings
pub struct SymbolManager {
    pub symbol_to_id: FxHashMap<String, u32>,
    pub id_to_symbol: FxHashMap<u32, String>,
    pub symbol_info: FxHashMap<u32, SymbolInfo>,
    pub assets: FxHashMap<u32, AssetInfo>,
}

impl Default for SymbolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolManager {
    pub fn new() -> Self {
        SymbolManager {
            symbol_to_id: FxHashMap::default(),
            id_to_symbol: FxHashMap::default(),
            symbol_info: FxHashMap::default(),
            assets: FxHashMap::default(),
        }
    }

    pub fn insert(&mut self, symbol: &str, id: u32) {
        self.insert_with_decimals(symbol, id, 2, 8); // Default: 2 for price, 8 for quantity
    }

    pub fn insert_with_decimals(
        &mut self,
        symbol: &str,
        id: u32,
        price_decimal: u32,
        quantity_decimal: u32,
    ) {
        self.symbol_to_id.insert(symbol.to_string(), id);
        self.id_to_symbol.insert(id, symbol.to_string());
        self.symbol_info.insert(
            id,
            SymbolInfo {
                symbol: symbol.to_string(),
                id,
                price_decimal,
                quantity_decimal,
            },
        );
    }

    pub fn get_id(&self, symbol: &str) -> Option<u32> {
        self.symbol_to_id.get(symbol).copied()
    }

    pub fn get_symbol(&self, id: u32) -> Option<&String> {
        self.id_to_symbol.get(&id)
    }

    pub fn get_symbol_info(&self, symbol: &str) -> Option<&SymbolInfo> {
        let id = self.get_id(symbol)?;
        self.symbol_info.get(&id)
    }

    pub fn get_symbol_info_by_id(&self, id: u32) -> Option<&SymbolInfo> {
        self.symbol_info.get(&id)
    }

    pub fn add_asset(&mut self, id: u32, symbol: &str, decimals: u32) {
        self.assets.insert(
            id,
            AssetInfo {
                id,
                symbol: symbol.to_string(),
                decimals,
            },
        );
    }

    pub fn get_asset_decimal(&self, asset_id: u32) -> Option<u32> {
        self.assets.get(&asset_id).map(|a| a.decimals)
    }

    /// Load initial state (simulating DB load)
    pub fn load_from_db() -> Self {
        let mut manager = SymbolManager::new();
        // BTC_USDT: price decimal 2 (e.g., 50000.12), quantity decimal 8 (e.g., 0.00000001 BTC)
        manager.insert_with_decimals("BTC_USDT", 0, 2, 8);
        // ETH_USDT: price decimal 2 (e.g., 3000.50), quantity decimal 8 (e.g., 0.00000001 ETH)
        manager.insert_with_decimals("ETH_USDT", 1, 2, 8);

        // Add Assets
        manager.add_asset(1, "BTC", 8);
        manager.add_asset(2, "USDT", 8);
        manager.add_asset(3, "ETH", 8);
        
        manager
    }
}
