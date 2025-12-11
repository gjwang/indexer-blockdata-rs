# Internal Transfer Out - Implementation Design

内部划转出账实现设计 (Spot → Funding)

**参见 API 定义:** `INTERNAL_TRANSFER_API.md`

---

## 📋 目录

- [1. 总体架构](#1-总体架构)
- [2. 账户体系](#2-账户体系)
- [3. 状态机设计](#3-状态机设计)
- [4. 数据流](#4-数据流)
- [5. 超时策略](#5-超时策略)
- [6. 容错机制](#6-容错机制)
- [7. 关键差异](#7-关键差异)

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
│  - Pre-check user spot balance (TB)                         │
│  - 创建 TB PENDING (锁定用户余额)                            │
│  - 发送请求到 UBSCore (via Aeron)                           │
│  - 网络错误时安全重试                                        │
│  - 根据 UBSCore 明确响应处理                                │
│  - 只在明确拒绝时 VOID                                       │
└─────────────────────────────────────────────────────────────┘
                          ↓ Aeron
┌─────────────────────────────────────────────────────────────┐
│                        UBSCore                              │
│  - 接收 transfer request                                    │
│  - 用户扣款操作                                              │
│  - 发送确认到 Settlement (via Kafka)                        │
│  - Dedup (基于 request_id)                                  │
└─────────────────────────────────────────────────────────────┘
                          ↓ Kafka
┌─────────────────────────────────────────────────────────────┐
│                      Settlement                             │
│  - POST_PENDING (完成资金流转到 funding)                     │
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
// Spot Account (用户现货账户)
let spot_account_id = tb_account_id(user_id, asset_id);
// 用途: 用户现货余额
// transfer_out: 扣款源（debits_pending）

// Funding Account (资金池)
let funding_account_id = tb_account_id(0, asset_id);
// user_id = 0
// 用途: Gateway 的资金池
// transfer_out: 入账目标（credits_pending）
```

### 2.2 资金流向

```
Transfer Out (划转出账):
  spot_account → funding_account

  目的: 将用户现货账户资金划转回资金池
  用途: 内部资金管理，例如用户申请外部提现前的准备

  - Gateway CREATE_PENDING: 锁定用户余额
    → spot.debits_pending += amount 🔒
    → funding.credits_pending += amount (未到账)

  - Settlement POST_PENDING: 完成划转
    → spot.debits_posted += amount
    → spot.debits_pending -= amount 🔓
    → funding.credits_posted += amount ✅
    → funding.credits_pending -= amount
```

---

## 3. 状态机设计

### 3.1 状态定义

```rust
pub enum TransferStatus {
    Requesting,  // Gateway 已持久化请求
    Pending,     // 用户余额已锁定（TB PENDING created）
    Success,     // 完成（UBSCore 扣款 + Settlement POST 成功）
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
      "from_account": {"account_type": "spot", "user_id": 3001, "asset": "USDT"},
      "to_account": {"account_type": "funding", "asset": "USDT"},
      "amount": "1000.00000000"
    }

T0: Gateway
    1. Pre-check spot_account balance (TB)
       └─ available >= amount? 否则 400 INSUFFICIENT_BALANCE
       └─ available = credits_posted - debits_posted - debits_pending
       └─ **自然防重**: debits_pending 已包含其他 pending 划转

    2. Insert ScyllaDB
       └─ status: "requesting"
       └─ from_account_type: "spot"
       └─ to_account_type: "funding"
       └─ **关键**: 先持久化，才能 lock user balance

    3. TB CREATE_PENDING (id: request_id)
       spot_account → funding_account
       └─ spot.debits_pending += amount 🔒
       └─ funding.credits_pending += amount (未到账)

    4. Update ScyllaDB
       └─ status: "pending"

    5. Aeron send to UBSCore (重试最多 20s)
       └─ 网络错误 → 重试
       └─ 明确拒绝 (INSUFFICIENT_BALANCE) → VOID + failed
       └─ 未知错误 → 保持 pending

    6. Return response
       └─ { request_id, status: "pending" }

UBSCore:
    7. 收到 Aeron 消息 (Dedup: request_id)

    8. 用户扣款 (内部逻辑)

    9. Kafka publish to Settlement
       └─ BalanceUpdateEvent

Settlement (Kafka path):
    10. 收到 BalanceUpdateEvent

    11. TB POST_PENDING (无限重试)
        └─ POST transfer (pending_id: request_id)
        └─ spot.debits_posted += amount
        └─ spot.debits_pending -= amount 🔓
        └─ funding.credits_posted += amount ✅
        └─ funding.credits_pending -= amount

    12. Update ScyllaDB
        └─ status: "success" ✅
```

### 4.2 Gateway Crash 恢复

```
场景: Gateway crash 在步骤 3-4 之间

实际状态:
  - ScyllaDB: status = "requesting"
  - TB: PENDING exists (用户余额已锁定)

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
   → 用户余额释放

禁止:
❌ Settlement 自动 VOID
❌ 超时自动 VOID
❌ 任何自动 VOID
```

**原因:**
```
场景: Settlement 自动 VOID
时间线:
  T1: Gateway CREATE_PENDING (user 锁定 1000)
  T2: Aeron 发送成功,但 UBSCore 处理慢
  T30min: Settlement 超时自动 VOID (user 释放)
  T31min: UBSCore 处理完成，用户扣款 -1000

结果:
  - 用户: -1000 ❌ (已扣款)
  - User balance: 已释放 ✅
  - 用户实际损失 1000 💸

风险: 资金灾难
```

**告警策略:**
- T30min: ⚠️ 告警 (用户余额锁定)
- T2h: 🚨 严重告警 (需人工核查)
- T24h: 🚨🚨 紧急告警，人工介入

---

## 6. 容错机制

### 6.1 Gateway 错误处理

```rust
// 明确业务拒绝 -> VOID
if let Some(business_error) = parse_business_error(&aeron_error) {
    match business_error.reason.as_str() {
        "INSUFFICIENT_BALANCE" | "INVALID_AMOUNT" | "USER_FROZEN" => {
            // 关键: 先 VOID TB
            let void_result = tb_client.void_transfer(request_id).await;

            match void_result {
                Ok(_) => {
                    // VOID 成功，更新 DB
                    db.update_status(request_id, TransferStatus::Failed).await?;
                    tracing::info!("✅ VOIDED rejected transfer: {}", request_id);
                }
                Err(e) => {
                    // ❌ VOID 失败
                    tracing::error!("🚨 VOID failed for {}: {}. User balance stuck!", request_id, e);
                    alert_critical(format!("VOID failed for {}: {}", request_id, e));
                    // 不更新 DB，保持 pending，让 Settlement 处理
                }
            }
        }
        _ => {
            // 不确定状态，保持 pending
            tracing::warn!("Uncertain error for {}, keeping pending", request_id);
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
                // 永不自动 VOID
            }
        }
    } else {
        // PENDING 消失，重新查询
        // 检查 spot 和 funding 账户状态
    }
}
```

---

## 7. 关键差异（transfer_out vs transfer_in）

### 7.1 账户操作方向

| 操作 | transfer_in (划转入) | transfer_out (划转出) |
|------|---------------------|----------------------|
| **Pre-check** | funding balance | **user spot balance** |
| **PENDING 源** | funding_account | **spot_account** |
| **PENDING 目标** | spot_account | **funding_account** |
| **锁定的账户** | funding.debits_pending | **spot.debits_pending** |
| **UBSCore 操作** | 用户入账 | **用户扣款** |
| **POST 完成** | spot.credits_posted++ | **funding.credits_posted++** |

### 7.2 业务拒绝原因

| transfer_in | transfer_out |
|-------------|--------------|
| USER_NOT_FOUND | USER_NOT_FOUND |
| INVALID_ASSET | INVALID_ASSET |
| - | **INSUFFICIENT_BALANCE** ⭐ |
| - | **USER_FROZEN** ⭐ |
| - | **DAILY_LIMIT_EXCEEDED** ⭐ |

### 7.3 风险点

| 风险 | transfer_in | transfer_out |
|------|-------------|--------------|
| 自动 VOID 后 UBSCore 处理 | 用户多得钱 💸 | **用户丢钱** 💸💸 |
| VOID 失败 | funding 锁定 | **用户余额锁定** ⚠️ |
| 严重程度 | 高 | **极高** 🚨 |

**transfer_out 更严格的要求:**
- ✅ 绝对不能自动 VOID
- ✅ 用户余额锁定必须告警
- ✅ 人工介入更严格审核
- ✅ 需要双重确认机制

---

## 8. 监控告警

### 8.1 关键指标

```
- transfer_out_requests_total{status}
- transfer_out_pending_age_seconds
- transfer_out_user_balance_locked_total
- transfer_out_void_failed_total
```

### 8.2 告警规则

```yaml
# User balance 锁定超过 30 分钟
- alert: TransferOutUserBalanceLocked
  expr: transfer_out_pending_age_seconds > 1800
  labels:
    severity: warning
  annotations:
    summary: "User balance locked for transfer_out"

# User balance 锁定超过 2 小时
- alert: TransferOutUserBalanceLockCritical
  expr: transfer_out_pending_age_seconds > 7200
  labels:
    severity: critical
  annotations:
    summary: "User balance locked > 2h"

# VOID 失败
- alert: TransferOutVoidFailed
  expr: transfer_out_void_failed_total > 0
  labels:
    severity: critical
  annotations:
    summary: "Transfer out VOID failed, user balance stuck!"
```

---

**设计版本**: v1.0
**最后更新**: 2025-12-12
**对应 API**: Spot → Funding
**参考**: INTERNAL_TRANSFER_API.md
