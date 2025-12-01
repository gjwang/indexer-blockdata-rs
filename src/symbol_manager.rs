use rustc_hash::FxHashMap;

/// Manages symbol-to-ID and ID-to-symbol mappings
pub struct SymbolManager {
    pub symbol_to_id: FxHashMap<String, u32>,
    pub id_to_symbol: FxHashMap<u32, String>,
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
        }
    }

    pub fn insert(&mut self, symbol: &str, id: u32) {
        self.symbol_to_id.insert(symbol.to_string(), id);
        self.id_to_symbol.insert(id, symbol.to_string());
    }

    pub fn get_id(&self, symbol: &str) -> Option<u32> {
        self.symbol_to_id.get(symbol).copied()
    }

    pub fn get_symbol(&self, id: u32) -> Option<&String> {
        self.id_to_symbol.get(&id)
    }

    /// Load initial state (simulating DB load)
    pub fn load_from_db() -> Self {
        let mut manager = SymbolManager::new();
        manager.insert("BTC_USDT", 0);
        manager.insert("ETH_USDT", 1);
        manager
    }
}
