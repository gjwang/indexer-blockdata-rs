# 代码结构分析 - Internal Transfer

**分析日期:** 2025-12-12 01:11
**目的:** 为实现 Internal Transfer 功能确定代码位置和结构

---

## 项目结构

```
src/
├── bin/                    # 二进制程序（services）
│   ├── settlement_service.rs
│   ├── gateway_server.rs
│   └── ...
├── models/                 # 数据模型
│   ├── api_response.rs     # API 响应模板
│   ├── balance_requests.rs  # ⚠️ 现有 BalanceRequest 定义（旧格式）
│   ├── events.rs
│   └── mod.rs
├── db/                     # 数据库访问层
│   ├── settlement_db.rs    # ⭐ 主要 DB 操作（2147 行）
│   ├── order_history_db.rs
│   └── mod.rs
├── gateway.rs              # Gateway API handler
└── ...

schema/
├── settlement_unified.cql  # ⭐ 统一 settlement schema
├── balance_ledger.cql      # 余额账本
├── active_orders.cql
└── ...
```

---

## 关键发现

### 1. 现有的BalanceRequest定义

**位置:** `src/models/balance_requests.rs`

**当前格式（旧）:**
```rust
pub enum Balance Request {
    TransferIn {
        request_id: u64,
        user_id: u64,
        asset_id: u32,
        amount: u64,
        timestamp: u64,
    },
    TransferOut { ... }
}
```

**需要更新为新的统一格式:**
根据设计文档，应该改为：
```rust
pub enum BalanceRequest {
    Transfer {
        request_id: u64,
        from_account: AccountType,
        to_account: AccountType,
        amount: i64,
    }
}
```

### 2. DB 层非常完善

**位置:** `src/db/settlement_db.rs`

**已有功能:**
- ✅ balance_ledger 操作（append-only）
- ✅ Batch insert
- ✅ Retry with backoff
- ✅ Prepared statements
- ✅ Slow query logging

**需要添加:**
- ❌ `balance_transfer_requests` 表的 CRUD

### 3. Schema 文件组织

**约定:**
- 每个表一个 .cql 文件
- 或者使用统一的 schema 文件

**需要创建:**
- `schema/internal_transfer.cql` 或
- 添加到 `schema/settlement_unified.cql`

---

## 参考实现

### 类似功能: balance_ledger

**表结构:** `schema/balance_ledger.cql`
``sql
CREATE TABLE balance_ledger (
    user_id bigint,
    asset_id int,
    seq bigint,
    delta_avail bigint,
    delta_frozen bigint,
    avail bigint,
    frozen bigint,
    event_type text,
    ref_id bigint,
    created_at bigint,
    PRIMARY KEY ((user_id, asset_id), seq)
) WITH CLUSTERING ORDER BY (seq DESC);
```

**代码:** `settlement_db.rs`
- INSERT_BALANCE_EVENT_CQL (line 196)
- SELECT_LATEST_BALANCE_CQL (line 207)
- BalanceLedgerEntry struct (line 234)

### API Response模板

**位置:** `src/models/api_response.rs`

**当前定义:**
```rust
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}
```

**可以直接使用** ✅

---

## 命名规范

### 文件命名
- ✅ `snake_case.rs`
- 例: `internal_transfer_types.rs`, `internal_transfer_db.rs`

### 结构体命名
- ✅ `PascalCase`
- 例: `AccountType`, `InternalTransferRequest`, `TransferStatus`

### 函数命名
- ✅ `snake_case`
- 例: `insert_transfer_request`, `update_transfer_status`

### 常量命名
- ✅ `UPPER_SNAKE_CASE`
- 例: `INSERT_TRANSFER_REQUEST_CQL`, `MAX_RETRIES`

---

## 测试策略

### 现有测试组织

**单元测试:**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_xxx() { ... }
}
```

**集成测试:**
```
tests/
└── xxx_test.rs
```

### Mock 框架

需要检查现有依赖中是否有 mock 框架

---

## 模块位置决定

### 新增文件位置

```
src/models/
└── internal_transfer_types.rs   # AccountType, TransferStatus
                                  # InternalTransferRequest,
                                  # InternalTransferData

src/db /
└── internal_transfer_db.rs      # TransferRequestRecord
                                  # insert_transfer_request()
                                  # update_transfer_status()

src/api/ (可能需要新建)
└── internal_transfer_handler.rs # API endpoint handler

schema/
└── internal_transfer.cql        # balance_transfer_requests 表
```

### 或者添加到现有文件

**选项 1:** 更新现有文件
- `src/models/balance_requests.rs` → 添加新类型
- `src/db/settlement_db.rs` → 添加新方法

**选项 2:** 新建独立文件（推荐）
- 更清晰的模块划分
- 便于测试和维护

---

## 依赖清单

### 已确认存在

- ✅ ScyllaDB session management
- ✅ serde (序列化/反序列化)
- ✅ anyhow (错误处理)
- ✅ tokio (异步运行时)

### 需要检查

还需要在 Step -1.2 中确认：
- TigerBeetle 客户端
- Aeron 消息系统
- Kafka integration
- Symbol Manager
- Request ID 生成器

---

## 下一步行动

### Step -1.2: 依赖检查

1. **TigerBeetle:**
   - 查找 TB 客户端初始化代码
   - 确认账户创建方式
   - 查看现有 TB 操作

2. **Aeron:**
   - 查找 Aeron 发送代码
   - 确认消息格式

3. **Kafka:**
   - 查找 Kafka producer/consumer
   - 确认 topic 命名

4. **Symbol Manager:**
   - 查找 symbol_manager 使用方式
   - 确认精度验证接口

5. **Request ID:**
   - 查找 request_id 生成方式
   - 确认 Snowflake 实现

---

## 结论

✅ **项目结构清晰，有很好的基础**

**建议实施路径:**
1. 新建独立模块文件（而非修改现有文件）
2. 参考 balance_ledger 的实现模式
3. 使用现有的 ApiResponse 模板
4. 遵循现有的命名和测试规范

**预计复杂度:** 中等
**关键风险:** 需要正确集成 TigerBeetle 和 Aeron

---

**完成时间:** 1 小时
**下一步:** Step -1.2 依赖检查
