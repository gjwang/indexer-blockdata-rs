# Service Development Standards

This document defines the **mandatory patterns** that all services in this project must follow.

## Table of Contents
1. [Security Requirements](#security-requirements) ⚠️ **CRITICAL**
2. [Configuration Management](#configuration-management)
3. [Logging Standards](#logging-standards)
4. [Boot Parameter Display](#boot-parameter-display)
5. [Complete Service Template](#complete-service-template)

---

## Security Requirements

### ⚠️ CRITICAL: Never Log Sensitive Data

**MANDATORY RULE:** Services MUST NEVER log sensitive data in full.

**Sensitive Data Includes:**
- Passwords
- API keys / Secret keys
- Private keys / Seed phrases
- Authentication tokens / JWT tokens
- Session IDs
- Credit card numbers
- Personal identification numbers (SSN, passport, etc.)
- Database connection strings with passwords
- Encryption keys
- OAuth secrets

### Allowed: Prefix/Suffix Logging

If you need to log sensitive data for debugging, log **ONLY** a small prefix or suffix:

**DO (Safe):**
```rust
// Log first 4 characters only
log::info!(target: LOG_TARGET, "API Key prefix: {}...", &api_key[..4]);

// Log last 4 characters only
log::info!(target: LOG_TARGET, "Token suffix: ...{}", &token[token.len()-4..]);

// Log length and prefix
log::debug!(target: LOG_TARGET, "Password length: {}, prefix: {}...", 
    password.len(), &password[..2]);

// Redact middle portion
log::info!(target: LOG_TARGET, "Connection: postgres://user:***@localhost/db");
```

**DON'T (Dangerous):**
```rust
// ❌ NEVER log full sensitive data
log::info!(target: LOG_TARGET, "API Key: {}", api_key);
log::debug!(target: LOG_TARGET, "Password: {}", password);
log::info!(target: LOG_TARGET, "Private key: {}", private_key);
log::error!(target: LOG_TARGET, "Auth failed with token: {}", token);

// ❌ NEVER log full config with secrets
log::debug!(target: LOG_TARGET, "Config: {:?}", config);  // May contain secrets!
```

### Helper Function Pattern

Create a helper function to safely log sensitive strings:

```rust
/// Safely log sensitive data by showing only prefix
fn safe_log_sensitive(data: &str, prefix_len: usize) -> String {
    if data.len() <= prefix_len {
        "*".repeat(data.len())
    } else {
        format!("{}...{}", &data[..prefix_len], "*".repeat(data.len() - prefix_len))
    }
}

// Usage
log::info!(target: LOG_TARGET, "API Key: {}", safe_log_sensitive(&api_key, 4));
// Output: "API Key: sk_t...*********************"
```

### Boot Parameters Security

When displaying boot parameters, **REDACT** sensitive fields:

**DO:**
```rust
// Redact database password
let db_url_safe = config.database_url.replace(
    &extract_password(&config.database_url),
    "***REDACTED***"
);
log::info!(target: LOG_TARGET, "  Database URL:     {}", db_url_safe);

// Show only key prefix
log::info!(target: LOG_TARGET, "  API Key:          {}...", &config.api_key[..4]);

// Don't log secret at all
log::info!(target: LOG_TARGET, "  Secret Key:       [CONFIGURED]");
```

**DON'T:**
```rust
// ❌ Never log full credentials
log::info!(target: LOG_TARGET, "  Database URL:     {}", config.database_url);
log::info!(target: LOG_TARGET, "  API Key:          {}", config.api_key);
```

### Configuration File Security

**NEVER** commit sensitive data to configuration files:

**DO:**
```yaml
# config/my_service_config.yaml
database:
  host: "localhost"
  port: 5432
  user: "app_user"
  # Password loaded from environment variable
  # Set via: export APP__DATABASE__PASSWORD=secret123

api:
  endpoint: "https://api.example.com"
  # API key loaded from environment variable
  # Set via: export APP__API__KEY=sk_live_abc123
```

**DON'T:**
```yaml
# ❌ NEVER commit secrets to config files
database:
  password: "super_secret_password_123"  # DANGEROUS!
  
api:
  key: "sk_live_abc123xyz789"  # DANGEROUS!
```

### Environment Variables for Secrets

**ALWAYS** use environment variables for sensitive data:

```rust
// Load secrets from environment
let api_key = std::env::var("API_KEY")
    .expect("API_KEY environment variable not set");

let db_password = std::env::var("DATABASE_PASSWORD")
    .expect("DATABASE_PASSWORD environment variable not set");

// Log safely
log::info!(target: LOG_TARGET, "API Key configured: {}...", &api_key[..4]);
log::info!(target: LOG_TARGET, "Database password: [CONFIGURED]");
```

### Debug Logging Security

Be especially careful with `debug!` and `trace!` logs:

```rust
// ❌ DANGEROUS: Debug logs might expose secrets
log::debug!(target: LOG_TARGET, "Request: {:?}", request);  // May contain auth headers!

// ✅ SAFE: Sanitize before logging
let safe_request = sanitize_request(&request);  // Remove auth headers
log::debug!(target: LOG_TARGET, "Request: {:?}", safe_request);
```

### Checklist: Security Review

Before deploying, verify:

- [ ] No passwords logged (even in debug/trace)
- [ ] No API keys logged in full (only prefix if needed)
- [ ] No private keys or seeds logged
- [ ] No tokens logged in full
- [ ] Boot parameters redact sensitive fields
- [ ] Configuration files don't contain secrets
- [ ] Secrets loaded from environment variables
- [ ] Debug logs sanitized (no auth headers, tokens, etc.)
- [ ] Error messages don't expose sensitive data

### Consequences of Logging Sensitive Data

**Why this matters:**
1. **Security Breach**: Logs may be stored in unsecured locations
2. **Compliance Violation**: GDPR, PCI-DSS, SOC2 violations
3. **Credential Theft**: Attackers can extract credentials from logs
4. **Legal Liability**: Data breaches can result in lawsuits
5. **Reputation Damage**: Loss of customer trust

**Remember: Logs are often the first place attackers look for credentials!**



## Configuration Management

### 1. Service-Specific Configuration Files

**Rule:** Each service MUST have its own configuration file.

**Naming Convention:**
```
config/{service_name}_config.yaml
```

**Examples:**
- `config/settlement_config.yaml`
- `config/matching_engine_config.yaml`
- `config/market_data_config.yaml`

### 2. Configuration Loading Pattern

**Required Code:**
```rust
use fetcher::configure;
use fetcher::logger::setup_logger;

fn main() {
    // Load service-specific configuration
    let config = configure::load_service_config("settlement_config")
        .expect("Failed to load settlement configuration");

    // Setup logger using config
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }
    
    // ... rest of service code
}
```

### 3. Configuration Hierarchy

Configuration is loaded in this order (later sources override earlier):

1. `config/config.yaml` - Base configuration
2. `config/{RUN_MODE}.yaml` - Environment-specific (dev/prod/test)
3. `config/{service}_config.yaml` - Service-specific (highest priority)
4. Environment variables with `APP__` prefix - Ultimate override

**Example:**
```bash
# Override log level via environment variable
APP__LOG_LEVEL=debug cargo run --bin settlement_service
```

### 4. Required Configuration Fields

Every service config file MUST include:

```yaml
# config/settlement_config.yaml
log_file: "log/settlement.log"
log_level: "info"              # trace, debug, info, warn, error, off
log_to_file: true

# Service-specific settings below
zeromq:
  settlement_port: 5557
  market_data_port: 5558
```

---

## Logging Standards

### 1. Custom Log Target (MANDATORY)

**Rule:** All services MUST use a custom log target for clean, readable logs.

**Required Pattern:**
```rust
// Define at the top of main.rs
const LOG_TARGET: &str = "settlement";  // Use short, clean name

fn main() {
    // ... config and logger setup ...
    
    // Use custom target in ALL log calls
    log::info!(target: LOG_TARGET, "Service started");
    log::error!(target: LOG_TARGET, "Error occurred: {}", err);
    log::debug!(target: LOG_TARGET, "Debug info: {:?}", data);
}
```

**Why:** This produces clean logs like `[settlement]` instead of `[settlement_service]`.

### 2. Log Levels Usage

Use log levels appropriately:

| Level | When to Use | Example |
|-------|-------------|---------|
| `error!` | Critical errors, failures | Gap detection, connection failures |
| `warn!` | Warnings, degraded state | High latency, retry attempts |
| `info!` | Normal operations | Service started, trade processed |
| `debug!` | Detailed diagnostics | Raw data dumps, state transitions |
| `trace!` | Very verbose debugging | Function entry/exit, loop iterations |

### 3. Log Format Standards

**DO:**
```rust
log::info!(target: LOG_TARGET, "Seq: {}, TradeID: {}, Price: {}", seq, id, price);
log::error!(target: LOG_TARGET, "Failed to connect: {}", error);
```

**DON'T:**
```rust
// ❌ No custom target
info!("Service started");

// ❌ Using println/eprintln for normal logs
println!("Trade processed");

// ❌ Verbose prefixes (target already shows service name)
log::info!(target: LOG_TARGET, "[Settlement] Trade processed");  // Redundant
```

### 4. RUST_LOG Environment Variable

All services automatically support the `RUST_LOG` environment variable:

```bash
# Set log level at runtime
RUST_LOG=debug cargo run --bin settlement_service

# Set to trace for maximum verbosity
RUST_LOG=trace ./settlement_service

# Disable logging
RUST_LOG=off ./settlement_service
```

---

## Boot Parameter Display

### MANDATORY: Boot Parameters Section

**Rule:** Every service MUST display its configuration at startup.

**Required Pattern:**
```rust
fn main() {
    // ... config loading and logger setup ...
    
    // Print boot parameters (MANDATORY)
    log::info!(target: LOG_TARGET, "=== {Service Name} Boot Parameters ===");
    log::info!(target: LOG_TARGET, "  Config File:      config/{service}_config.yaml");
    log::info!(target: LOG_TARGET, "  Log File:         {}", config.log_file);
    log::info!(target: LOG_TARGET, "  Log Level:        {}", config.log_level);
    log::info!(target: LOG_TARGET, "  Log to File:      {}", config.log_to_file);
    // Add service-specific parameters here
    log::info!(target: LOG_TARGET, "  ZMQ Endpoint:     {}", endpoint);
    log::info!(target: LOG_TARGET, "===========================================");
    
    log::info!(target: LOG_TARGET, "{Service Name} started.");
    
    // ... service logic ...
}
```

**Example Output:**
```
2025-12-04 16:20:34.699 [INFO] [settlement] - === Settlement Service Boot Parameters ===
2025-12-04 16:20:34.699 [INFO] [settlement] -   Config File:      config/settlement_config.yaml
2025-12-04 16:20:34.699 [INFO] [settlement] -   Log File:         log/settlement.log
2025-12-04 16:20:34.699 [INFO] [settlement] -   Log Level:        info
2025-12-04 16:20:34.699 [INFO] [settlement] -   Log to File:      true
2025-12-04 16:20:34.699 [INFO] [settlement] -   ZMQ Endpoint:     tcp://localhost:5557
2025-12-04 16:20:34.699 [INFO] [settlement] - ===========================================
2025-12-04 16:20:34.699 [INFO] [settlement] - Settlement Service started.
```

**Why:** This makes it immediately obvious what configuration is active, helping with:
- Debugging configuration issues
- Verifying correct environment
- Production troubleshooting

---

## Complete Service Template

Here's a complete template for creating a new service:

```rust
use fetcher::configure;
use fetcher::logger::setup_logger;

// Define custom log target (use short, clean name)
const LOG_TARGET: &str = "my_service";

fn main() {
    // 1. Load service-specific configuration
    let config = configure::load_service_config("my_service_config")
        .expect("Failed to load my_service configuration");

    // 2. Setup logger
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        return;
    }

    // 3. Extract service-specific config
    let my_config = config.my_service_settings.expect("Service config missing");

    // 4. Print boot parameters (MANDATORY)
    log::info!(target: LOG_TARGET, "=== My Service Boot Parameters ===");
    log::info!(target: LOG_TARGET, "  Config File:      config/my_service_config.yaml");
    log::info!(target: LOG_TARGET, "  Log File:         {}", config.log_file);
    log::info!(target: LOG_TARGET, "  Log Level:        {}", config.log_level);
    log::info!(target: LOG_TARGET, "  Log to File:      {}", config.log_to_file);
    // Add your service-specific parameters
    log::info!(target: LOG_TARGET, "  Port:             {}", my_config.port);
    log::info!(target: LOG_TARGET, "  Workers:          {}", my_config.workers);
    log::info!(target: LOG_TARGET, "===========================================");

    log::info!(target: LOG_TARGET, "My Service started.");

    // 5. Service initialization
    // ... your service logic here ...

    // 6. Main event loop
    log::info!(target: LOG_TARGET, "Entering main event loop...");
    loop {
        // Use custom target in all log calls
        log::debug!(target: LOG_TARGET, "Processing event");
        
        match process_event() {
            Ok(result) => {
                log::info!(target: LOG_TARGET, "Event processed: {:?}", result);
            }
            Err(e) => {
                log::error!(target: LOG_TARGET, "Failed to process event: {}", e);
            }
        }
    }
}

fn process_event() -> Result<String, String> {
    // Your logic here
    Ok("success".to_string())
}
```

---

## Configuration File Template

Create `config/{service}_config.yaml`:

```yaml
# Service-specific configuration for {Service Name}
# This file has highest priority and overrides config.yaml and {env}.yaml

# Logging configuration (REQUIRED)
log_file: "log/{service}.log"
log_level: "info"              # trace, debug, info, warn, error, off
log_to_file: true

# Service-specific settings
my_service_settings:
  port: 8080
  workers: 4
  timeout_ms: 5000
  
# Optional: Override base config if needed
# kafka:
#   broker: "localhost:9092"
```

---

## Checklist for New Services

Before deploying a new service, verify:

**Security (CRITICAL):**
- [ ] ⚠️ No passwords, API keys, or secrets logged in full
- [ ] ⚠️ Sensitive data only logged as prefix (first 4 chars) if needed
- [ ] ⚠️ Boot parameters redact sensitive fields
- [ ] ⚠️ Configuration files don't contain secrets (use env vars)
- [ ] ⚠️ Debug logs sanitized (no auth headers, tokens, etc.)

**Configuration:**
- [ ] Service has its own `config/{service}_config.yaml` file
- [ ] Uses `load_service_config("{service}_config")` to load config
- [ ] Secrets loaded from environment variables

**Logging:**
- [ ] Defines `const LOG_TARGET: &str = "{service}";` with short name
- [ ] All log calls use `log::info!(target: LOG_TARGET, ...)` pattern
- [ ] No use of `println!` or `eprintln!` for normal logging (only for pre-logger errors)
- [ ] Appropriate log levels used (error for failures, info for operations, debug for diagnostics)
- [ ] Service logs to `log/{service}.log`

**Boot Parameters:**
- [ ] Displays boot parameters at startup
- [ ] Boot parameters include: config file, log file, log level, log-to-file, and service-specific settings
- [ ] Sensitive fields in boot parameters are redacted


---

## Benefits of This Pattern

1. **Isolation**: Each service has independent configuration
2. **Clarity**: Clean, short log targets (`[settlement]` not `[settlement_service]`)
3. **Debuggability**: Boot parameters show exact configuration
4. **Flexibility**: Easy to override via environment variables
5. **Consistency**: All services follow same pattern
6. **Production-Ready**: Professional logging suitable for production systems

---

## Examples in Codebase

**Reference Implementation:**
- `src/bin/settlement_service.rs` - Complete example following all patterns
- `config/settlement_config.yaml` - Configuration file example
- `src/logger.rs` - Logger implementation
- `src/configure.rs` - Configuration loading logic

**Study these files to understand the complete pattern.**

---

## Questions?

If you have questions about these patterns, refer to:
1. The settlement service implementation (`src/bin/settlement_service.rs`)
2. The logging documentation in `src/logger.rs`
3. The configuration documentation in `src/configure.rs`

**Remember: These are MANDATORY patterns. All services must follow them.**
