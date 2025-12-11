# Internal Transfer In - Implementation Design

内部划转入账实现设计 (Funding → Spot)

**参见 API 定义:** `INTERNAL_TRANSFER_API.md`

---

## 📋 目录

- [1. 总体架构](#1-总体架构)
- [2. 账户体系](#2-账户体系)
- [3. 状态机设计](#3-状态机设计)
- [4. 数据流](#4-数据流)
- [5. 超时策略](#5-超时策略)
- [6. 容错机制](#6-容错机制)

---

## 1. 总体架构

### 1.0 设计原则

**资金安全铁律:**
1. ✅ **request_id 全局唯一** - 使用 SnowflakeGen<RandomSequence> 生成
2. ✅ **状态只能前进** - 使用状态机 + CAS 保证
3. ✅ **TB 是真相源** - 所有状态可从 TB 重建
4. ✅ **先持久化再锁定** - 防止丢失 request 记录
5. ✅ **明确失败才 VOID** - 不确定状态不操作，等待恢复
6. ✅ **Settlement 可恢复一切** - 扫描中间状态，从 TB 同步

### 1.1 组件职责

```
┌─────────────────────────────────────────────────────────────┐
│                         Client                              │
│  - 发起 internal_transfer 请求                              │
│  - 轮询查询状态                                              │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│                        Gateway                              │
│  - Pre-check funding balance (TB)                           │
│  - 创建 TB PENDING (锁定 funding)                            │
│  - 发送请求到 UBSCore (via Aeron)                           │
│  - 网络错误时安全重试                                        │
│  - 根据 UBSCore 明确响应处理                                │
│  - 只在明确拒绝时 VOID                                       │
└─────────────────────────────────────────────────────────────┘
                          ↓ Aeron
┌─────────────────────────────────────────────────────────────┐
│                        UBSCore                              │
│  - 接收 transfer request                                    │
│  - 用户入账操作                                              │
│  - 发送确认到 Settlement (via Kafka)                        │
│  - Dedup (基于 request_id)                                  │
└─────────────────────────────────────────────────────────────┘
                          ↓ Kafka
┌─────────────────────────────────────────────────────────────┐
│                      Settlement                             │
│  - POST_PENDING (完成资金流转)                               │
│  - 无限重试直到成功                                          │
│  - 扫描未完成状态 (requesting/pending)                       │
│  - 从 TB 恢复状态                                            │
│  - 永不自动 VOID                                             │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. 账户体系

### 2.1 TigerBeetle 账户

```rust
// Funding Account (资金池)
let funding_account_id = tb_account_id(0, asset_id);
// user_id = 0
// 用途: Gateway 的资金池
// transfer_in: 扣款源（debits_pending）

// Spot Account (用户现货账户)
let spot_account_id = tb_account_id(user_id, asset_id);
// 用途: 用户现货余额
// transfer_in: 入账目标（credits_pending）
```

### 2.2 资金流向

```
Transfer In (划转入账):
  funding_account → spot_account

  - Gateway CREATE_PENDING: 锁定 funding 余额
    → funding.debits_pending += amount 🔒
    → spot.credits_pending += amount (未到账)

  - Settlement POST_PENDING: 完成划转
    → funding.debits_posted += amount
    → funding.debits_pending -= amount 🔓
    → spot.credits_posted += amount ✅
    → spot.credits_pending -= amount
```

---

## 3. 状态机设计

### 3.1 状态定义

```rust
pub enum TransferStatus {
    Requesting,  // Gateway 已持久化请求
    Pending,     // Funding 余额已锁定（TB PENDING created）
    Success,     // 完成（UBSCore 入账 + Settlement POST 成功）
    Failed,      // 失败（明确拒绝或人工 VOID）
}
```

### 3.2 状态转换

```
requesting → pending    (Gateway 锁定 / Settlement 恢复)
requesting → success    (Settlement 恢复，极端情况)
requesting → failed     (Gateway 拒绝 / Settlement 清理)

pending → success       (Settlement POST 成功)
pending → failed        (人工 VOID，Gateway 明确拒绝)

success → X             (终态)
failed → X              (终态)
```

---

## 4. 数据流

### 4.1 正常流程

```
T0: Client → Gateway
    POST /api/v1/user/internal_transfer
    {
      "from_account": {"account_type": "funding", "asset": "USDT"},
      "to_account": {"account_type": "spot", "user_id": 3001, "asset": "USDT"},
      "amount": "1000.00000000"
    }

T0: Gateway
    1. Pre-check funding_account balance (TB)
       └─ available >= amount? 否则 503
       └─ 防超卖: 依赖 TB CREATE_PENDING 原子性

    2. Insert ScyllaDB
       └─ status: "requesting"
       └─ from_account_type: "funding"
       └─ to_account_type: "spot"
       └─ **关键**: 先持久化，才能 lock funding

    3. TB CREATE_PENDING (id: request_id)
       funding_account → spot_account
       └─ funding.debits_pending += amount 🔒
       └─ spot.credits_pending += amount

    4. Update ScyllaDB
       └─ status: "pending"

    5. Aeron send to UBSCore (重试最多 20s)
       └─ 网络错误 → 重试
       └─ 明确拒绝 → VOID + failed
       └─ 未知错误 → 保持 pending

    6. Return response
       └─ { request_id, status: "pending" }

UBSCore:
    7. 收到 Aeron 消息 (Dedup: request_id)

    8. 用户入账 (内部逻辑)

    9. Kafka publish to Settlement
       └─ BalanceUpdateEvent

Settlement (Kafka path):
    10. 收到 BalanceUpdateEvent

    11. TB POST_PENDING (无限重试)
        └─ POST transfer (pending_id: request_id)
        └─ funding.debits_posted += amount
        └─ funding.debits_pending -= amount 🔓
        └─ spot.credits_posted += amount ✅
        └─ spot.credits_pending -= amount

    12. Update ScyllaDB
        └─ status: "success" ✅
```

### 4.2 Gateway Crash 恢复

```
场景: Gateway crash 在步骤 3-4 之间

实际状态:
  - ScyllaDB: status = "requesting"
  - TB: PENDING exists (funding 已锁定)

Settlement 扫描 (T60s 后):
  1. 查询 ScyllaDB: status = "requesting"

  2. 查询 TB: lookup_transfers([request_id])
     └─ 找到 PENDING

  3. 恢复状态:
     └─ requesting → pending ✅

  4. 继续正常流程
     └─ 等待 UBSCore 确认
```

---

## 5. 超时策略

### 5.1 超时常量

```rust
const GATEWAY_TIMEOUT_MS: u64 = 20 * 1000;            // 20 秒
const SETTLEMENT_WAIT_MS: u64 = 60 * 1000;            // 60 秒
const PENDING_ALERT_MS: u64 = 30 * 60 * 1000;         // 30 分钟
const PENDING_CRITICAL_MS: u64 = 2 * 3600 * 1000;    // 2 小时
const REQUESTING_CLEANUP_MS: u64 = 24 * 3600 * 1000;  // 24 小时
```

### 5.2 PENDING 状态处理

**永不自动 VOID:**

```
PENDING 状态只有两种结束方式:

1. ✅ UBSCore 发送成功确认
   → Settlement POST_PENDING → success

2. ✅ 人工确认失败
   → 管理员手动 VOID → failed

禁止:
❌ Settlement 自动 VOID
❌ 超时自动 VOID
```

**告警策略:**
- T30min: ⚠️ 告警 (PENDING 超时)
- T2h: 🚨 严重告警
- T24h: 🚨🚨 紧急告警，人工介入

---

## 6. 容错机制

### 6.1 Gateway 错误处理

```rust
// 明确业务拒绝 -> VOID
if let Some(business_error) = parse_business_error(&aeron_error) {
    match business_error.reason.as_str() {
        "USER_NOT_FOUND" | "INVALID_ASSET" => {
            // 先 VOID TB
            tb_client.void_transfer(request_id).await?;

            // VOID 成功后更新 DB
            db.update_status(request_id, TransferStatus::Failed).await?;
        }
        _ => {
            // 不确定状态，保持 pending
        }
    }
}
```

### 6.2 Settlement 扫描恢复

```rust
// 扫描频率: 5 秒

// 处理 "requesting"
if status == "requesting" {
    if let Some(tb_transfer) = tb_client.lookup_transfer(request_id).await? {
        match tb_transfer.flags {
            PENDING => db.update_status(request_id, Pending).await?,
            POSTED => db.update_status(request_id, Success).await?,
            VOIDED => db.update_status(request_id, Failed).await?,
        }
    } else if age >= 24h {
        db.update_status(request_id, Failed).await?;
    }
}

// 处理 "pending"
if status == "pending" {
    if let Some(tb_transfer) = tb_client.lookup_transfer(request_id).await? {
        match tb_transfer.flags {
            POSTED => db.update_status(request_id, Success).await?,
            VOIDED => db.update_status(request_id, Failed).await?,
            PENDING => {
                if age >= 30min { alert_warning(); }
                if age >= 2h { alert_critical(); }
            }
        }
    } else {
        // PENDING 消失，重新查询 TB
        // 检查 funding 和 spot 账户状态
    }
}
```

---

**设计版本**: v1.0
**最后更新**: 2025-12-12
**对应 API**: Funding → Spot
**参考**: INTERNAL_TRANSFER_API.md
