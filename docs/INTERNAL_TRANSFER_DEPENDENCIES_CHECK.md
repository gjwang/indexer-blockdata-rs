# 依赖检查清单 - Internal Transfer

**检查日期:** 2025-12-12 01:15
**目的:** 确认所有外部依赖就绪

---

## ✅ TigerBeetle

**位置:** `src/ubs_core/tigerbeetle.rs`

**客户端:**
```rust
use tigerbeetle_unofficial::{Client, Transfer};
use tigerbeetle_unofficial::transfer::Flags as TransferFlags;

// 初始化
let client = Client::new(cluster_id, addresses).map_err(...)?;
```

**账户创建:**
```rust
pub fn tb_account_id(user_id: u64, asset_id: u32) -> u128 {
    ((user_id as u128) << 64) | (asset_id as u128)
}

// Funding 账户 (user_id = FUNDING_POOL_ID, 需要定义)
let funding_account_id = tb_account_id(0, asset_id);

// Spot 账户
let spot_account_id = tb_account_id(user_id, asset_id);
```

**现有操作:**
- ✅ `client.create_accounts()` - 创建账户
- ✅ `client.create_transfers()` - 创建转账
- ✅ `Transfer::new().with_flags(TransferFlags::PENDING)` - PENDING 转账
- ✅ `Transfer::new().with_flags(TransferFlags::POST_PENDING_TRANSFER)` - POST
- ✅ `Transfer::new().with_flags(TransferFlags::VOID_PENDING_TRANSFER)` - VOID

**特殊账户:**
```rust
pub const EXCHANGE_OMNIBUS_ID_PREFIX: u64 = u64::MAX;          // 全局资金池
pub const HOLDING_ACCOUNT_ID_PREFIX: u64 = u64::MAX - 1;       // 锁定账户
pub const REVENUE_ACCOUNT_ID_PREFIX: u64 = u64::MAX - 2;       // 收入账户
```

**Funding Account 定义:**
根据设计，Funding account 应该使用 user_id = 0：
```rust
pub const FUNDING_POOL_ID: u64 = 0;
```

**余额查询:**
❌ **缺失**: `get_available_balance()` 方法
需要实现或找到现有实现

---

## ✅ ScyllaDB

**位置:** `src/db/settlement_db.rs`

**Session 初始化:**
```rust
let session = SessionBuilder::new()
    .known_nodes(&config.hosts)
    .connection_timeout(Duration::from_millis(config.connection_timeout_ms))
    .build()
    .await?;
```

**Schema 位置:** `schema/settlement_unified.cql`

**Migration 工具:**
手动执行 CQL 文件

**现有操作:**
- ✅ `insert_trade()` - 单条插入
- ✅ `insert_batch()` - 批量插入
- ✅ `retry_with_backoff()` - 重试机制
- ✅ Prepared statements

**需要添加:**
- ❌ `insert_transfer_request()` - 插入划转请求
- ❌ `update_transfer_status()` - 更新状态
- ❌ `get_transfer_by_id()` - 查询划转

---

## ⚠️ Aeron

**搜索结果:**
未找到明确的 Aeron 使用代码

**可能位置:**
- `src/bin/ubscore_aeron_service.rs` - 存在但未查看详细
- 可能使用自定义封装

**需要确认:**
- Aeron 发送消息的接口
- 消息格式定义
- UBSCore 通信协议

**替代方案:**
如果 Aeron 暂不可用，可以先使用内存队列 mock

---

## ⚠️ Kafka

**搜索结果:**
未找到 Kafka 相关代码

**需要确认:**
- Kafka producer/consumer 配置
- Topic 命名规范
- 消息序列化方式

**替代方案:**
Settlement 部分可以暂时不实现，先完成 Gateway → UBSCore 的流程

---

## ✅ Symbol Manager

**位置:** `src/symbol_manager.rs`

**使用方式:**
```rust
let manager = SymbolManager::load_from_db();

// 获取资产信息
manager.get_asset_id("USDT") // Some(2)
manager.get_asset_decimal(asset_id) // Some(8)
manager.get_asset_display_decimals(asset_id) // Some(2)
```

**现有资产:**
```rust
asset_id=1: BTC (8 decimals, 3 display)
asset_id=2: USDT (8 decimals, 2 display)
asset_id=3: ETH (8 decimals, 4 display)
```

**精度验证:**
可以使用 `get_asset_decimal()` 和 `display_decimals` 进行验证

**注意:**
设计文档中提到 `min_transfer_amount`，但 Symbol Manager 中没有。
需要：
1. 添加到 SymbolManager
2. 或者使用固定值（如 0.00000001）

---

## ✅ Request ID 生成

**现有实现:**

**SnowflakeGen:**
未找到明确的 SnowflakeGen 实现

**现有 ID 生成:**
```rust
// src/ubs_core/tigerbeetle.rs
fn generate_transfer_id() -> u128 {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u128;

    let seq = TRANSFER_SEQUENCE.fetch_add(1, Ordering::SeqCst) as u128;
    (timestamp_ms << 64) | seq
}
```

**可以使用:**
- 现有的 `generate_transfer_id()` 返回 u128
- 或者实现简化的 Snowflake (u64)

**建议:** 使用 u64 版本的 Snowflake：
```rust
pub fn generate_request_id() -> u64 {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let seq = TRANSFER_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    (timestamp_ms << 20) | (seq & 0xFFFFF) // 44 bits timestamp + 20 bits sequence
}
```

---

## 📋 依赖总结

| 依赖 | 状态 | 说明 |
|------|------|------|
| **TigerBeetle** | ✅ 就绪 | Client, Transfer 操作完整 |
| **ScyllaDB** | ✅ 就绪 | Session, 现有操作可参考 |
| **Aeron** | ⚠️ 需确认 | 未找到使用代码，可先 mock |
| **Kafka** | ⚠️ 需确认 | 未找到，Settlement 可后期实现 |
| **Symbol Manager** | ✅ 就绪 | 可用于精度验证 |
| **Request ID** | ✅ 可实现 | 参考现有 ID 生成 |

---

## 🎯 实施优先级

### 阶段 1: 最小可行 (MVP)

**包含:**
1. ✅ TigerBeetle 操作
2. ✅ ScyllaDB 操作
3. ✅ Symbol Manager 验证
4. ⚠️ Mock Aeron（内存队列）
5. ❌ 暂不实现 Kafka/Settlement

**目标:** Gateway 接收请求 → TB 锁定 → Mock UBSCore 确认

### 阶段 2: 完整集成

**添加:**
1. ⚠️ 真实 Aeron 集成
2. ⚠️ Kafka producer/consumer
3. ⚠️ Settlement service

---

## 🚀 下一步行动

### Step -1.3: 测试环境准备

1. ✅ 确定 mock 框架
2. ✅ 准备测试数据
3. ✅ 配置测试DB
4. ✅ CI/CD 检查

**预估时间:** 30 分钟

---

**完成时间:** 30 分钟
**状态:** ✅ 依赖基本就绪，可以开始实施
**下一步:** Step -1.3 测试环境准备
