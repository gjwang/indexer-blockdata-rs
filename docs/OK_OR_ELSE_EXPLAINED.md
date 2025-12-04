# Understanding `.ok_or_else()` in Rust

## The Code in Question

```rust
pub fn add_order(&mut self, symbol: &str, side: Side, price: u64, quantity: u64) -> Result<u64, String> {
    let book = self.order_books.get_mut(symbol)
        .ok_or_else(|| format!("Symbol {} not found", symbol))?;
    
    book.add_order(symbol, side, price, quantity)
}
```

## What Does `.ok_or_else()` Do?

`.ok_or_else()` converts an `Option<T>` into a `Result<T, E>`.

### Step-by-Step Breakdown

#### Step 1: `get_mut()` returns `Option`
```rust
self.order_books.get_mut(symbol)
// Returns: Option<&mut OrderBook>
// - Some(&mut OrderBook) if symbol exists
// - None if symbol doesn't exist
```

#### Step 2: `.ok_or_else()` converts to `Result`
```rust
.ok_or_else(|| format!("Symbol {} not found", symbol))
// Converts Option to Result:
// - Some(value) → Ok(value)
// - None → Err(error_from_closure)
```

#### Step 3: `?` operator propagates errors
```rust
... ?
// If Result is Err, returns early from function
// If Result is Ok(value), unwraps to value
```

## Two Scenarios

### ✅ Scenario 1: Symbol EXISTS
```rust
// Input
symbol = "BTC_USDT"
self.order_books contains "BTC_USDT"

// Step 1: get_mut
self.order_books.get_mut("BTC_USDT")
→ Some(&mut OrderBook)

// Step 2: ok_or_else
.ok_or_else(|| format!("Symbol {} not found", "BTC_USDT"))
→ Ok(&mut OrderBook)
// The closure || format!(...) is NOT called!

// Step 3: ?
→ Unwraps to &mut OrderBook

// Result: book = &mut OrderBook
// Function continues to line 237
```

### ❌ Scenario 2: Symbol DOES NOT EXIST
```rust
// Input
symbol = "DOGE_USDT"
self.order_books does NOT contain "DOGE_USDT"

// Step 1: get_mut
self.order_books.get_mut("DOGE_USDT")
→ None

// Step 2: ok_or_else
.ok_or_else(|| format!("Symbol {} not found", "DOGE_USDT"))
→ Err("Symbol DOGE_USDT not found")
// The closure IS called to create the error message

// Step 3: ?
→ Returns Err("Symbol DOGE_USDT not found") immediately
// Function exits here, doesn't reach line 237

// Result: Function returns Err(String)
```

## Why Use `.ok_or_else()` Instead of `.ok_or()`?

### `.ok_or()` - Eager Evaluation
```rust
// BAD: Creates error message even if not needed!
.ok_or(format!("Symbol {} not found", symbol))
// format! is called ALWAYS, even when symbol exists
```

### `.ok_or_else()` - Lazy Evaluation
```rust
// GOOD: Only creates error message when needed
.ok_or_else(|| format!("Symbol {} not found", symbol))
// Closure || {...} only called when Option is None
```

**Performance Difference:**
- `.ok_or()`: Creates error string EVERY time (wasteful)
- `.ok_or_else()`: Creates error string ONLY on error (efficient)

## Complete Flow Example

```rust
// Success case
engine.add_order("BTC_USDT", Side::Buy, 100, 10)
→ get_mut("BTC_USDT") → Some(&mut book)
→ ok_or_else(...) → Ok(&mut book)
→ ? → &mut book
→ book.add_order(...) → Ok(order_id)
→ Returns Ok(order_id)

// Error case
engine.add_order("INVALID", Side::Buy, 100, 10)
→ get_mut("INVALID") → None
→ ok_or_else(...) → Err("Symbol INVALID not found")
→ ? → early return
→ Returns Err("Symbol INVALID not found")
```

## Alternative Ways to Write This

### Without `.ok_or_else()`
```rust
pub fn add_order(&mut self, symbol: &str, ...) -> Result<u64, String> {
    match self.order_books.get_mut(symbol) {
        Some(book) => book.add_order(symbol, side, price, quantity),
        None => Err(format!("Symbol {} not found", symbol)),
    }
}
```

### Using `if let`
```rust
pub fn add_order(&mut self, symbol: &str, ...) -> Result<u64, String> {
    if let Some(book) = self.order_books.get_mut(symbol) {
        book.add_order(symbol, side, price, quantity)
    } else {
        Err(format!("Symbol {} not found", symbol))
    }
}
```

### Current Idiomatic Way (Most Concise)
```rust
pub fn add_order(&mut self, symbol: &str, ...) -> Result<u64, String> {
    let book = self.order_books.get_mut(symbol)
        .ok_or_else(|| format!("Symbol {} not found", symbol))?;
    book.add_order(symbol, side, price, quantity)
}
```

## Key Takeaways

1. **`.ok_or_else()`** converts `Option<T>` → `Result<T, E>`
2. **Closure `|| {...}`** is only executed when `Option` is `None`
3. **`?` operator** propagates errors up the call stack
4. **Lazy evaluation** with `_else` variants is more efficient
5. This pattern is **idiomatic Rust** for Option → Result conversion

## When the Closure is Called

```rust
.ok_or_else(|| format!("Symbol {} not found", symbol))
              ↑
              This closure (|| {...}) is ONLY called when:
              - The Option is None
              - We need to create an error value
              
If Option is Some:
- Closure is NEVER called
- No string formatting happens
- Very efficient!
```

This is why `.ok_or_else()` is better than `.ok_or()` - it avoids unnecessary work! 🚀
