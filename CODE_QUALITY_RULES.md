# Code Quality Rules

**MANDATORY**: All code must follow these quality standards.

## 1. No Hardcoded Values in Production Code

### ❌ NEVER Hardcode These

#### SQL/CQL/Database Queries
Hardcoded queries make refactoring difficult and error-prone.

```rust
// ❌ BAD - Hardcoded query inline
session.prepare("INSERT INTO users (id, name) VALUES (?, ?)").await?;

// ✅ GOOD - Extract to module-level constant
const INSERT_USER_CQL: &str = "INSERT INTO users (id, name) VALUES (?, ?)";
session.prepare(INSERT_USER_CQL).await?;
```

**Why?**
- Single source of truth for queries
- Easy to find and update all queries
- Syntax highlighting in some editors
- Easier to test and mock

#### API Endpoints/URLs
```rust
// ❌ BAD - Hardcoded URL
let url = "https://api.example.com/v1/users";

// ✅ GOOD - Use configuration or constant
const API_BASE_URL: &str = "https://api.example.com/v1";
let url = format!("{}/users", API_BASE_URL);

// ✅ BETTER - From configuration
let url = format!("{}/users", config.api_base_url);
```

#### Magic Numbers
```rust
// ❌ BAD - What does 86400 mean?
let days = timestamp / 86400;

// ✅ GOOD - Named constant with clear meaning
const SECONDS_PER_DAY: i64 = 86400;
let days = timestamp / SECONDS_PER_DAY;
```

```rust
// ❌ BAD - Unclear buffer size
let mut buffer = Vec::with_capacity(1024);

// ✅ GOOD - Named constant
const DEFAULT_BUFFER_SIZE: usize = 1024;
let mut buffer = Vec::with_capacity(DEFAULT_BUFFER_SIZE);
```

#### Port Numbers
```rust
// ❌ BAD - Hardcoded port
let addr = "127.0.0.1:8080";

// ✅ GOOD - From configuration
let addr = format!("127.0.0.1:{}", config.server_port);
```

#### File Paths
```rust
// ❌ BAD - Hardcoded path
let path = "/var/log/app.log";

// ✅ GOOD - From configuration
let path = &config.log_file;
```

#### Sensitive Data (CRITICAL!)
```rust
// ❌ BAD - NEVER EVER hardcode credentials
let api_key = "sk-1234567890abcdef";
let password = "secret123";

// ✅ GOOD - Always use environment variables
let api_key = std::env::var("API_KEY")?;
let password = std::env::var("DB_PASSWORD")?;
```

**CRITICAL**: Never commit secrets to version control!

---

## 2. Where to Put Constants

### Module-Level Constants
For constants used within a single module:

```rust
// At the top of the file, after imports
use anyhow::Result;
use scylla::Session;

// Constants section
const INSERT_TRADE_CQL: &str = "INSERT INTO trades ...";
const MAX_BATCH_SIZE: usize = 100;
const RETRY_ATTEMPTS: u32 = 3;

pub struct MyService {
    // ...
}
```

### Shared Constants Module
For constants used across multiple modules:

```rust
// src/constants.rs
pub const SECONDS_PER_DAY: i64 = 86400;
pub const MAX_RETRY_ATTEMPTS: u32 = 3;
pub const DEFAULT_TIMEOUT_MS: u64 = 5000;

// Usage in other files
use crate::constants::SECONDS_PER_DAY;
```

### Configuration Files
For environment-specific or deployment-specific values:

```yaml
# config/config.yaml
database:
  host: "localhost"
  port: 9042
  keyspace: "settlement"

api:
  base_url: "https://api.example.com/v1"
  timeout_ms: 5000
```

---

## 3. Naming Conventions for Constants

### Use SCREAMING_SNAKE_CASE
```rust
const MAX_CONNECTIONS: usize = 100;
const API_BASE_URL: &str = "https://api.example.com";
const SECONDS_PER_DAY: i64 = 86400;
```

### Be Descriptive
```rust
// ❌ BAD - Unclear
const LIMIT: usize = 100;
const TIMEOUT: u64 = 5000;

// ✅ GOOD - Clear purpose
const MAX_BATCH_SIZE: usize = 100;
const CONNECTION_TIMEOUT_MS: u64 = 5000;
```

### Include Units in Name
```rust
const CONNECTION_TIMEOUT_MS: u64 = 5000;  // milliseconds
const CACHE_TTL_SECONDS: u64 = 3600;      // seconds
const MAX_FILE_SIZE_MB: usize = 100;      // megabytes
```

---

## 4. Benefits of This Approach

### Maintainability
- Change value in one place
- Easy to find all uses with IDE search
- Refactoring is safer

### Readability
- Self-documenting code
- Clear intent
- No need to guess what `86400` means

### Security
- No secrets in code
- Environment-specific values in config
- Easier to audit

### Testing
- Easy to override constants in tests
- Mock values without changing production code
- Feature flags become trivial

---

## 5. Examples from Our Codebase

### Good Example: settlement_db.rs
```rust
// CQL Statements
const INSERT_TRADE_CQL: &str = "
    INSERT INTO settled_trades (
        trade_date, output_sequence, trade_id, match_seq,
        buy_order_id, sell_order_id,
        buyer_user_id, seller_user_id,
        price, quantity, base_asset, quote_asset,
        buyer_refund, seller_refund, settled_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
";

// Later in code
let stmt = session.prepare(INSERT_TRADE_CQL).await?;
```

### Good Example: Time Constants
```rust
const SECONDS_PER_DAY: i64 = 86400;
const MILLISECONDS_PER_SECOND: i64 = 1000;

let trade_date = (now_ts / SECONDS_PER_DAY) as i32;
let settled_at = Utc::now().timestamp_millis();
```

---

## 6. Exceptions (Rare!)

### When Hardcoding is OK

**Test Code**
```rust
#[test]
fn test_parse_user() {
    let json = r#"{"id": 1, "name": "Alice"}"#;  // OK in tests
    // ...
}
```

**Error Messages**
```rust
// OK - Error messages can be inline
return Err(anyhow!("Failed to connect to database"));
```

**Log Messages**
```rust
// OK - Log messages can be inline
log::info!("Server started on port {}", port);
```

**Type Definitions**
```rust
// OK - Type aliases and trait bounds
type UserId = u64;
```

---

## 7. Enforcement

### Code Review Checklist
- [ ] No hardcoded SQL/CQL queries
- [ ] No hardcoded URLs or endpoints
- [ ] No magic numbers (use named constants)
- [ ] No hardcoded credentials or secrets
- [ ] Constants have clear, descriptive names
- [ ] Units are included in constant names where applicable

### IDE Hints
Look for these patterns in code review:
- String literals in `.prepare()`, `.query()`, `.execute()`
- Numeric literals used multiple times
- URLs starting with `http://` or `https://`
- Paths starting with `/` or `C:\`

---

## 8. Migration Guide

If you find hardcoded values in existing code:

1. **Extract to constant**:
   ```rust
   // Before
   session.prepare("INSERT INTO ...").await?;

   // After
   const INSERT_QUERY: &str = "INSERT INTO ...";
   session.prepare(INSERT_QUERY).await?;
   ```

2. **Move to config if environment-specific**:
   ```rust
   // Before
   let url = "https://api.example.com";

   // After (in config.yaml)
   api_base_url: "https://api.example.com"

   // In code
   let url = &config.api_base_url;
   ```

3. **Use environment variables for secrets**:
   ```rust
   // Before
   let key = "secret123";

   // After
   let key = std::env::var("API_KEY")
       .expect("API_KEY environment variable not set");
   ```

## 9. General Coding Standards

1. **Language**: Comments must be in English.
2. **Simplicity**: Prioritize concise, readable, and practical code. Avoid over-engineering.
3. **Complexity**: Minimize cyclomatic complexity; maximize reusability.
4. **Architecture**: Employ modular design and appropriate patterns.
5. **Modifications**: Minimize side effects and changes to unrelated modules.
6. **API Standard**: Server actions must return `{ status: number, msg: string, data: Option<T> }`.
---


## Summary

**Golden Rule**: If you're typing a string literal or number that represents a configuration value, query, URL, or constant - **STOP** and extract it to a named constant or configuration.

**Ask yourself**:
- Will this value ever change?
- Is this value used in multiple places?
- Does this value have meaning that should be documented?
- Is this value environment-specific?

If **YES** to any → Extract it!

---

## 10. Pre-Commit Standards

**MANDATORY**: Before committing any code, you MUST run:
```bash
cargo fmt
cargo check
```
This ensures code consistency and prevents build errors.

