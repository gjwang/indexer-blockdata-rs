# Private Channel Implementation - Quick Start

## 📁 Files Created

### Core Libraries
- `src/centrifugo_publisher.rs` - Publisher service for balance/order/position updates
- `src/centrifugo_auth.rs` - JWT token generation and validation

### API Server
- `src/bin/private_channel_api.rs` - Axum-based API server with endpoints

### Client & Config
- `private_channel_demo.html` - Interactive demo client
- `config_private_channels.json` - Centrifugo configuration with user# channels

## 🚀 Quick Start

### 1. Add Dependencies to Cargo.toml

```toml
[dependencies]
# Existing dependencies...
jsonwebtoken = "9"
chrono = "0.4"

# For API server (private_channel_api binary)
axum = "0.7"
tower-http = { version = "0.5", features = ["cors"] }
```

### 2. Start Centrifugo with New Config

```bash
docker restart centrifugo
# Or if using custom config:
# docker run -v $(pwd)/config_private_channels.json:/centrifugo/config.json ...
```

### 3. Run the API Server

```bash
cargo run --bin private_channel_api
```

The API will start on `http://localhost:3000` with endpoints:
- `POST /api/auth/login` - Login
- `GET /api/centrifugo/token` - Get Centrifugo token
- `POST /api/test/publish-balance` - Test balance update
- `POST /api/test/publish-order` - Test order update

### 2. Open Demo Client
```bash
# Start HTTP server
python3 -m http.server 8080

# Open in browser
open http://localhost:8080/web_clients/private_channel_demo.html
```

1. Login with any username/password
2. Click "Connect to Centrifugo"
3. You'll be subscribed to `user#12345:balance`, `user#12345:orders`, `user#12345:positions`

### 5. Test Publishing

```bash
# Publish balance update
curl -X POST http://localhost:3000/api/test/publish-balance \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": "12345",
    "asset": "BTC",
    "available": 1.5,
    "locked": 0.2
  }'

# Publish order update
curl -X POST http://localhost:3000/api/test/publish-order \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": "12345",
    "order_id": "order_001",
    "symbol": "BTC/USDT",
    "side": "buy",
    "status": "filled",
    "price": 50000,
    "quantity": 0.1
  }'
```

## 🔧 Integration Example

### In Your Application

```rust
use fetcher::centrifugo_publisher::{CentrifugoPublisher, BalanceUpdate};

let publisher = CentrifugoPublisher::new(
    "http://localhost:8000/api".to_string(),
    "your_api_key".to_string(),
);

// When user balance changes
let balance = BalanceUpdate {
    asset: "BTC".to_string(),
    available: 1.5,
    locked: 0.2,
    total: 1.7,
    timestamp: chrono::Utc::now().timestamp(),
};

publisher.publish_balance_update("12345", balance).await?;
```

## 🔐 Security Notes

- User can ONLY subscribe to `user#{their_id}:*` channels
- JWT `sub` claim must match the user ID in the channel name
- Centrifugo automatically enforces this validation

## 📚 Full Documentation

See `private_channel_architecture.md` for complete architecture details.
