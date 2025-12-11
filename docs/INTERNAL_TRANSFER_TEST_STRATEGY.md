# 测试策略 - Internal Transfer

**制定日期:** 2025-12-12 01:18
**目的:** 定义测试框架、数据和覆盖率要求

---

## Mock 框架

### 已有依赖

从 `Cargo.toml` 检查：
- ❌ 未找到 mockall
- ❌ 未找到 mockito

### Dev Dependencies

```toml
[dev-dependencies]
tempfile = \"3.23.0\"
tower = { version = \"0.5.2\", features = [\"util\"] }
```

### 建议添加

```toml
[dev-dependencies]
# ... 现有依赖
mockall = \"0.13\"        # Mock 框架
tokio-test = \"0.4\"      # 异步测试工具
```

### 替代方案（不添加新依赖）

使用 手工 mock：
```rust
// Mock TigerBeetle Client
struct MockTbClient {
    // ...
}

impl MockTbClient {
    fn new() -> Self { ... }
    async fn create_transfers(&self, transfers: Vec<Transfer>) -> Result<()> {
        // Mock implementation
        Ok(())
    }
}
```

---

## 测试数据 Fixtures

### 创建位置

```
tests/
└── fixtures/
    └── internal_transfer.rs
```

### 示例数据

```rust
// tests/fixtures/internal_transfer.rs

use fetcher::models::internal_transfer_types::*;

// 测试常量
pub const TEST_USER_ID: u64 = 9999;
pub const TEST_REQUEST_ID: u64 = 1234567890;
pub const TEST_ASSET_ID: u32 = 2; // USDT
pub const TEST_AMOUNT: i64 = 100_000_000; // 1.00 USDT (8 decimals)

// Funding 账户
pub fn sample_funding_account() -> AccountType {
    AccountType::Funding {
        asset: \"USDT\".to_string(),
    }
}

// Spot 账户
pub fn sample_spot_account(user_id: u64) -> AccountType {
    AccountType::Spot {
        user_id,
        asset: \"USDT\".to_string(),
    }
}

// InternalTransferRequest
pub fn sample_transfer_in_request() -> InternalTransferRequest {
    InternalTransferRequest {
        from_account: sample_funding_account(),
        to_account: sample_spot_account(TEST_USER_ID),
        amount: rust_decimal::Decimal::new(100_000_000, 8), // 1.00 USDT
    }
}

pub fn sample_transfer_out_request() -> InternalTransferRequest {
    InternalTransferRequest {
        from_account: sample_spot_account(TEST_USER_ID),
        to_account: sample_funding_account(),
        amount: rust_decimal::Decimal::new(50_000_000, 8), // 0.50 USDT
    }
}

// TransferRequestRecord
pub fn sample_transfer_record() -> TransferRequestRecord {
    TransferRequestRecord {
        request_id: TEST_REQUEST_ID,
        from_account_type: \"funding\".to_string(),
        from_user_id: None,
        from_asset_id: TEST_ASSET_ID,
        to_account_type: \"spot\".to_string(),
        to_user_id: Some(TEST_USER_ID as i64),
        to_asset_id: TEST_ASSET_ID,
        amount: TEST_AMOUNT,
        status: \"requesting\".to_string(),
        created_at: 1702345678000,
        updated_at: 1702345678000,
        pending_transfer_id: None,
        posted_transfer_id: None,
        processor: None,
        error_message: None,
    }
}
```

---

## 测试覆盖率要求

### 工具

**推荐:** `cargo-tarpaulin`

```bash
# 安装
cargo install cargo-tarpaulin

# 运行
cargo tarpaulin --out Html --output-dir coverage/
```

### 覆盖率目标

| 模块 | 最低覆盖率 | 说明 |
|------|-----------|------|
| 核心逻辑 | 100% | 状态转换、验证、TB 操作 |
| DB 操作 | 90% | CRUD, 状态更新 |
| API Handler | 80% | 请求处理、错误响应 |
| 工具函数 | 90% | ID 生成、格式转换 |
| **总体** | **85%** | 项目最低要求 |

---

## 测试分类

### 1. 单元测试

**位置:** 在模块文件中

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_type_serialization() {
        let account = AccountType::Funding {
            asset: \"USDT\".to_string(),
        };

        let json = serde_json::to_string(&account).unwrap();
        assert!(json.contains(\"\\\"account_type\\\":\\\"funding\\\"\"));
    }

    #[tokio::test]
    async fn test_validate_transfer_request() {
        let req = sample_transfer_in_request();
        assert!(validate_transfer_request(&req).is_ok());
    }
}
```

### 2. 集成测试

**位置:** `tests/` 目录

```rust
// tests/internal_transfer_integration_test.rs

use fetcher::db::internal_transfer_db::InternalTransferDb;
use fetcher::models::internal_transfer_types::*;

#[tokio::test]
async fn test_insert_and_get_transfer_request() {
    // Setup test DB
    let db = setup_test_db().await;

    // Insert
    let record = sample_transfer_record();
    db.insert_transfer_request(record).await.unwrap();

    // Get
    let result = db.get_transfer_by_id(TEST_REQUEST_ID).await.unwrap();
    assert!(result.is_some());

    // Cleanup
    cleanup_test_db(&db).await;
}
```

### 3. Mock 测试

**TigerBeetle Mock:**

```rust
struct MockTbClient {
    balance: std::sync::Arc<std::sync::Mutex<i64>>,
}

impl MockTbClient {
    fn new(initial_balance: i64) -> Self {
        Self {
            balance: Arc::new(Mutex::new(initial_balance)),
        }
    }

    async fn get_available_balance(&self, _account_id: u128) -> Result<i64> {
        Ok(*self.balance.lock().unwrap())
    }

    async fn create_pending_transfer(&self, amount: i64) -> Result<()> {
        let mut bal = self.balance.lock().unwrap();
        if *bal >= amount {
            *bal -= amount;
            Ok(())
        } else {
            Err(anyhow::anyhow!(\"Insufficient balance\"))
        }
    }
}
```

---

## 测试隔离

### 原则

1. **每个测试独立**
   - 不依赖其他测试的状态
   - 使用不同的 request_id

2. **Mock 外部依赖**
   - TigerBeetle → MockTbClient
   - Aeron → Mock Channel
   - Kafka → Mock Producer

3. **清理测试数据**
   - 测试后删除 DB 记录
   - 重置 mock 状态

### 测试 Helper

```rust
// tests/common/mod.rs

pub async fn setup_test_db() -> SettlementDb {
    // 连接测试 DB
    // 或使用内存 DB
}

pub async fn cleanup_test_db(db: &SettlementDb) {
    // 删除测试数据
}

pub fn generate_test_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(10000);
    CO UNTER.fetch_add(1, Ordering::SeqCst)
}
```

---

## 测试数据库

### 本地 ScyllaDB

**Docker Compose:**

```yaml
# docker-compose.test.yml
version: '3.8'

services:
  scylla-test:
    image: scylladb/scylla:latest
    ports:
      - \"9043:9042\"
    environment:
      - SCYLLA_CLUSTER_NAME=test_cluster
    volumes:
      - ./schema:/schema
```

**启动:**
```bash
docker-compose -f docker-compose.test.yml up -d scylla-test

# 等待启动
sleep 10

# 创建 keyspace 和表
docker-compose exec scylla-test cqlsh -f /schema/settlement_unified.cql
```

### 替代方案

使用内存数据结构模拟：
```rust
struct InMemoryDb {
    transfers: Arc<Mutex<HashMap<u64, TransferRequestRecord>>>,
}
```

---

## CI/CD 配置检查

### 查找 CI 配置

```bash
ls -la .github/workflows/ 2>/dev/null || echo \"No GitHub Actions\"
ls -la .gitlab-ci.yml 2>/dev/null || echo \"No GitLab CI\"
```

### 建议 CI 步骤

```yaml
# .github/workflows/test.yml
name: Test

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Start ScyllaDB
        run: docker-compose -f docker-compose.test.yml up -d

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Run tests
        run: cargo test

      - name: Test coverage
        run: |
          cargo install cargo-tarpaulin
          cargo tarpaulin --out Xml

      - name: Upload coverage
        uses: codecov/codecov-action@v3
```

---

## 执行计划

### Step 1: 创建 Fixtures (10分钟)

```bash
mkdir -p tests/fixtures
# 创建 tests/fixtures/internal_transfer.rs
```

### Step 2: 配置 Mock (10分钟)

```rust
// src/mocks/mod.rs
pub mod tigerbeetle;
pub mod aeron;
```

### Step 3: 编写第一个测试 (10分钟)

```rust
#[test]
fn test_account_type_json() {
    // 验证 JSON 序列化
}
```

---

## 总结

✅ **测试框架:** 使用内置测试 + 手工 mock
✅ **测试数据:** 创建 fixtures 模块
✅ **覆盖率:** 目标 85%，核心 100%
✅ **隔离:** 独立测试 + Mock 依赖
✅ **CI:** 建议添加 GitHub Actions

**预估时间:** 30 分钟
**状态:** ✅ 策略明确，可以开始实施
**下一步:** Step 0.1 数据结构定义
