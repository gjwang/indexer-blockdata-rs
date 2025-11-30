# External Symbol Mapping Refactoring

## Overview
Refactored the `MatchingEngine` to be a **pure, ID-based execution core**. All symbol string-to-ID mapping logic has been encapsulated in a **`SymbolManager` struct**, enforcing a strict separation of concerns.

## Architecture

### 1. `MatchingEngine` (The Core)
- **State**: `Vec<Option<OrderBook>>`
- **Responsibility**: Efficient order matching using integer IDs.
- **No Mapping**: Does NOT store `String` -> `usize` maps.
- **API**:
    - `register_symbol(id: usize, symbol: String)`: Allocates slot for ID.
    - `add_order(symbol_id: usize, order_id: u64, ...)`: Adds order to book at ID.
    - `print_order_book(id: usize)`: Prints book at ID.

### 2. `OrderBook` (The Book)
- **State**:
    - `bids`, `asks`: Order storage.
    - `active_order_ids`: `HashSet<u64>` for uniqueness validation.
- **Validation**: Rejects duplicate `order_id`s for active orders.

### 3. `SymbolManager` (The Manager)
- **State**:
    - `symbol_to_id: HashMap<String, usize>` (Forward lookup)
    - `id_to_symbol: HashMap<usize, String>` (Reverse lookup)
- **Responsibility**:
    - Loading symbols from Database (`load_from_db`).
    - Managing bidirectional mappings.
    - Providing `insert` and `query` methods.
- **API**:
    - `insert(symbol: &str, id: usize)`
    - `get_id(symbol: &str) -> Option<usize>`
    - `get_symbol(id: usize) -> Option<&String>`

## Benefits

1.  **Separation of Concerns**: Engine focuses on matching, Manager focuses on metadata.
2.  **Clean API**: `SymbolManager` provides a dedicated interface for symbol operations.
3.  **Performance**: Engine remains lightweight and fast.
4.  **Robustness**: Duplicate order IDs are prevented.

## Usage Example

```rust
// 1. Load Symbols via Manager
let mut symbol_manager = SymbolManager::load_from_db();

// 2. Initialize Engine
let mut engine = MatchingEngine::new();
for (symbol, &id) in &symbol_manager.symbol_to_id {
    engine.register_symbol(id, symbol.clone()).unwrap();
}

// 3. Trading
let btc_id = symbol_manager.get_id("BTC_USDT").unwrap();
// Pass external Order ID (e.g., 1001)
engine.add_order(btc_id, 1001, Side::Buy, 100, 10).unwrap();
```

## Validation
- Verified `BTC_USDT` (ID 0) and `ETH_USDT` (ID 1) loading via Manager.
- Verified dynamic registration of `SOL_USDT` (ID 5).
- Verified gap handling (ID 2 returns error).
- Verified bidirectional lookup via Manager.
- Verified duplicate order ID rejection.
