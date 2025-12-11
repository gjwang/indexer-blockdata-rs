# Transfer Out (内部划转) Design

完整的用户账户到资金池的**内部划转**（Transfer Out）设计文档，与 Transfer In 对称且同样严谨。

**重要说明:**
- `transfer_out`: 内部划转，从 user_trading_account → funding_account
- 这是交易所内部的账户管理，用于不同用途账户之间的资金划转
- **不是外部提现**
- 外部提现使用 `withdraw` 术语
- 本文档与 `transfer_in` 完全对称

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
1. ✅ **request_id 全局唯一** - 使用 SnowflakeGen<RandomSequence> 生成，绝对唯一
2. ✅ **状态只能前进** - 使用状态机 + CAS 保证
3. ✅ **TB 是真相源** - 所有状态可从 TB 重建
4. ✅ **先持久化再锁定** - 防止丢失 request 记录
5. ✅ **明确失败才 VOID** - 不确定状态不操作，等待恢复
6. ✅ **Settlement 可恢复一切** - 扫描中间状态，从 TB 同步

### 1.1 组件职责

```
┌─────────────────────────────────────────────────────────────┐
│                         Client                              │
│  - 发起 transfer_out 请求                                    │
│  - 轮询查询状态                                              │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│                        Gateway                              │
│  - Pre-check user balance (TB)                              │
│  - 创建 TB PENDING (锁定用户余额)                            │
│  - 发送请求到 UBSCore (via Aeron)                           │
│  - 网络错误时安全重试                                        │
│  - 根据 UBSCore 明确响应处理（caller 责任）                 │
│  - 只在明确拒绝时 VOID                                       │
└─────────────────────────────────────────────────────────────┘
                          ↓ Aeron
┌─────────────────────────────────────────────────────────────┐
│                        UBSCore                              │
│  【黑盒服务 - Caller 无需关心内部逻辑】                       │
│  - 接收 transfer request                                    │
│  - 内部扣款逻辑（UBSCore 责任）                              │
│  - 发送明确响应（成功/失败/错误）                            │
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
│  - 清理孤儿记录 (24小时)                                     │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 职责边界

**Gateway:**
- ✅ 锁定用户余额（CREATE_PENDING）
- ✅ 发送请求到 UBSCore
- ✅ 处理 UBSCore 明确响应
- ✅ 失败时 VOID（先 VOID，再更新 DB）
- ❌ 不关心 UBSCore 内部扣款逻辑

**UBSCore:**
- ✅ 接收并处理 transfer request
- ✅ 内部扣款逻辑（UBSCore 负责）
- ✅ 返回明确响应（成功/拒绝/错误）
- ✅ 成功后发送确认到 Settlement
- ❌ Caller 无需知道内部实现

**Settlement:**
- ✅ 接收 UBSCore 确认
- ✅ POST_PENDING 完成流转到 funding
- ✅ 扫描和恢复中间状态
- ✅ 从 TB 同步状态
- ❌ 永不自动 VOID

---

## 2. 账户体系

### 2.1 TigerBeetle 账户

```rust
// User Trading Account (用户交易账户)
let user_account_id = tb_account_id(user_id, asset_id);
// 用途: 用户在交易系统内的余额
// transfer_out: 扣款源（debits_pending）

// Funding Account (Gateway 资金池)
let funding_account_id = tb_account_id(FUNDING_USER_ID, asset_id);
// FUNDING_USER_ID = 0
// 用途: Gateway 的资金池
// transfer_out: 入账目标（credits_pending）
```

### 2.2 资金流向

```
Transfer Out (内部划转):
  user_trading_account → funding_account

  目的: 将用户交易账户的资金划转回资金池
  用途: 内部资金管理，例如用户申请外部提现前的准备

  - Gateway CREATE_PENDING: 锁定用户余额
    → user.debits_pending += amount 🔒
    → funding.credits_pending += amount (未到账)

  - Settlement POST_PENDING: 完成划转
    → user.debits_posted += amount
    → user.debits_pending -= amount 🔓
    → funding.credits_posted += amount ✅
    → funding.credits_pending -= amount
```

---

## 3. 状态机设计

### 3.1 状态定义（与 transfer_in 相同）

```rust
pub enum TransferStatus {
    /// Gateway 已持久化请求，预检查通过
    Requesting,

    /// 用户余额已锁定（TB PENDING created）
    Pending,

    /// 完成（UBSCore 扣款 + Settlement POST 成功）
    Success,

    /// 失败（明确拒绝或人工 VOID）
    Failed,
}
```

### 3.2 状态转换（与 transfer_in 相同）

```
requesting → pending    (Gateway 锁定 / Settlement 恢复)
requesting → success    (Settlement 恢复，极端情况)
requesting → failed     (Gateway 拒绝 / Settlement 清理)

pending → success       (Settlement POST 成功)
pending → failed        (人工 VOID，Gateway 明确拒绝)

success → X             (终态，不允许)
failed → X              (终态，不允许)
```

---

## 4. 数据流

### 4.1 transfer_out (内部划转) - 正常流程

```
T0: Client → Gateway
    POST /api/v1/user/transfer_out
    { user_id, asset, amount }

    说明: 用户请求将资金从 trading account 划转到 funding account

T0: Gateway
    1. Pre-check user_account balance (TB)
       └─ available >= amount? 否则 400 INSUFFICIENT_BALANCE
       └─ **关键**: 检查用户余额，不是 funding
       └─ available = credits_posted - debits_posted - debits_pending
       └─ **自然防重**: debits_pending 已包含其他 pending 划转
       └─ 无需额外检查，TB PENDING 机制已足够

    2. Insert ScyllaDB
       └─ status: "requesting"
       └─ direction: "out"
       └─ **关键**: 必须先持久化，才能 lock user balance
       └─ **原因**: 防止 crash 后丢失 request 记录

    3. TB CREATE_PENDING (id: request_id)
       user_trading_account → funding_account
       └─ user.debits_pending += amount 🔒
       └─ funding.credits_pending += amount (未到账)

    4. Update ScyllaDB
       └─ status: "pending"

    5. Aeron send to UBSCore (重试最多 20s)
       └─ 网络错误 → 重试
       └─ 明确拒绝 (INSUFFICIENT_BALANCE) → VOID + failed
       └─ 未知错误 → 保持 pending

    6. Return response
       └─ { request_id }

T0-T20s: Gateway 重试 Aeron (最多 20秒)

T20s: Gateway 超时退出
      └─ status 保持 "pending"

UBSCore:
    7. 收到 Aeron 消息 (Dedup: request_id)

    8. 用户扣款 (TB)
       └─ UBSCore 内部扣款逻辑
       └─ Caller 无需关心

    9. Kafka publish to Settlement
       └─ BalanceUpdateEvent

Settlement (Kafka path):
    10. 收到 BalanceUpdateEvent

    11. TB POST_PENDING (无限重试)
        └─ POST transfer (pending_id: request_id)
        └─ user.debits_posted += amount
        └─ user.debits_pending -= amount 🔓
        └─ funding.credits_posted += amount ✅
        └─ funding.credits_pending -= amount

    12. Update ScyllaDB
        └─ status: "success" ✅

Client:
    13. Poll /api/v1/transfer/status/{request_id}
        └─ 100ms 间隔
        └─ 最多 10s
        └─ 返回 "success"
```

### 4.2 Gateway Crash 恢复流程

```
场景: Gateway crash 在步骤 3-4 之间

实际状态:
  - ScyllaDB: status = "requesting" (未更新)
  - TB: PENDING exists (用户余额已锁定)

Settlement 扫描 (T60s 后):
  1. 查询 ScyllaDB: status = "requesting", direction = "out"

  2. 查询 TB: lookup_transfers([request_id])
     └─ 找到 PENDING

  3. 恢复状态:
     └─ requesting → pending ✅

  4. 继续正常流程
     └─ 等待 UBSCore 或人工介入
```

---

## 5. 超时策略

### 5.1 超时常量（与 transfer_in 相同）

```rust
const GATEWAY_TIMEOUT_MS: u64 = 20 * 1000;            // 20 秒
const SETTLEMENT_WAIT_MS: u64 = 60 * 1000;            // 60 秒
const PENDING_ALERT_MS: u64 = 30 * 60 * 1000;         // 30 分钟（告警）
const PENDING_CRITICAL_MS: u64 = 2 * 3600 * 1000;    // 2 小时（严重告警）
const REQUESTING_CLEANUP_MS: u64 = 24 * 3600 * 1000;  // 24 小时
```

### 5.2 时间线

```
T0         Gateway 开始处理
T0-20s     Gateway 重试 Aeron (最多 20 秒)
T20s       Gateway 超时退出
T60s       Settlement 开始扫描 (等待 60 秒，确保 Gateway 已退出)
T60s-∞     等待 UBSCore 确认（永不自动 VOID）
T30min     PENDING 告警（⚠️ 等待超时，用户余额锁定）
T2h        PENDING 严重告警（🚨 需人工核查）
T24h       requesting 清理 → failed (✅ 无余额锁定)
```

### 5.3 PENDING 状态处理策略

**关键安全原则: 永不自动 VOID**

```
PENDING 状态的唯一两种结束方式:

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

---

## 6. 容错机制

### 6.1 Gateway 错误处理

```
Aeron send 错误分类:

1. 网络错误 (timeout, connection lost, buffer full)
   → 安全重试 (UBSCore 有 dedup)
   → 最多 20 秒
   → 失败后保持 "pending"，交给 Settlement

2. 明确业务拒绝 (INSUFFICIENT_BALANCE, INVALID_AMOUNT)
   → VOID TB PENDING
   → 更新 status: "failed"
   → 返回错误
   → 用户余额释放 ✅

3. 未知错误
   → 不 VOID (不确定 UBSCore 是否已处理)
   → 保持 "pending"
   → 交给 Settlement 处理
```

### 6.2 Settlement 扫描恢复

```
扫描频率: 5 秒

处理 "requesting" 状态:
  1. 查询 TB: lookup_transfers([request_id])

  2. 如果找到 PENDING:
     └─ 恢复: requesting → pending

  3. 如果找到 POSTED:
     └─ 恢复: requesting → success

  4. 如果找到 VOIDED:
     └─ 恢复: requesting → failed

  5. 如果未找到:
     └─ age < 24h: 继续等待
     └─ age >= 24h: 清理 → failed (无余额锁定，安全)

处理 "pending" 状态:
  1. 查询 TB: lookup_transfers([pending_id])

  2. 如果 PENDING:
     └─ age < 30min: 继续等待
     └─ age 30min-2h: ⚠️ 告警（但不 VOID）
     └─ age 2h-24h: 🚨 严重告警
     └─ age >= 24h: 🚨🚨 紧急告警
     └─ **永不自动 VOID**

  3. 如果 POSTED:
     └─ 同步: pending → success

  4. 如果 VOIDED:
     └─ 同步: pending → failed
     └─ （只能是人工 VOID 的结果）

  5. 如果消失（lookup 未找到）:
     └─ PENDING 消失只可能是状态前进了
     └─ 重新查询 TB transfers（可能已 POST 或 VOID）
     └─ 检查 user 和 funding 账户状态
     └─ 根据账户状态同步 DB:
        - 如果 user.debits_posted 增加 → success
        - 如果无变化 → 可能已 VOID
     └─ 如果仍无法确定 → 告警，等待人工核查
```

### 6.3 Gateway 明确拒绝时的 VOID

**唯一允许 Gateway 自动 VOID 的场景:**

```rust
// UBSCore 明确的业务拒绝
if let Some(business_error) = parse_business_error(&aeron_error) {
    match business_error.reason.as_str() {
        "INSUFFICIENT_BALANCE" | "INVALID_AMOUNT" | "USER_FROZEN" => {
            // ✅ 确认 UBSCore 拒绝

            // 关键：先 VOID，再更新 DB
            let void_transfer = Transfer {
                id: generate_new_id(),
                pending_id: request_id,
                flags: TransferFlags::VOID_PENDING_TRANSFER,
                ..Default::default()
            };

            match tb_client.create_transfers(&[void_transfer]).await {
                Ok(_) => {
                    // VOID 成功，更新 DB
                    db.transition_transfer_status(
                        request_id,
                        TransferStatus::Pending,
                        TransferStatus::Failed,
                        TransitionReason::GatewayRejected(
                            business_error.reason.clone()
                        ),
                        None,
                    ).await?;

                    tracing::info!("✅ VOIDED rejected withdrawal: {}", request_id);
                }
                Err(e) => {
                    // ❌ VOID 失败
                    tracing::error!(
                        "🚨 VOID failed for {}: {}. User balance stuck!",
                        request_id, e
                    );

                    alert_critical(format!(
                        "VOID failed for rejected withdrawal {}: {}. \
                         User balance locked!",
                        request_id, e
                    ));

                    // 不更新 DB 状态，保持 pending
                    // 让 Settlement 扫描处理
                }
            }
        }
        _ => {
            // ⚠️ 其他错误，不确定状态，不 VOID
            tracing::warn!(
                "Uncertain error for {}: {}. Keeping pending.",
                request_id, business_error.reason
            );
        }
    }
}
```

**操作顺序:**
1. ✅ 先执行 TB VOID
2. ✅ VOID 成功 → 更新 DB status = failed，用户余额释放
3. ❌ VOID 失败 → 不更新 DB，保持 pending
4. ✅ Settlement 扫描会处理卡住的情况

### 6.4 VOID 决策表

| 场景 | 可否 VOID | 原因 |
|------|----------|------|
| UBSCore 明确拒绝 (INSUFFICIENT_BALANCE) | ✅ Yes (Gateway) | 确认未扣款 |
| UBSCore 明确拒绝 (INVALID_AMOUNT) | ✅ Yes (Gateway) | 确认未扣款 |
| UBSCore 明确拒绝 (USER_FROZEN) | ✅ Yes (Gateway) | 确认未扣款 |
| PENDING 超时 (任何时长) | ❌ No | 不知道 UBSCore 状态 |
| Aeron 网络错误 | ❌ No | 可能已发送 |
| Aeron 未知错误 | ❌ No | 不确定状态 |
| UBSCore 错误 | ❌ No | 可能已扣款 |
| 人工确认失败 | ✅ Yes (手动) | 人工核查后确认 |

---

## 7. 关键差异（transfer_out vs transfer_in）

### 7.1 账户操作方向

| 操作 | transfer_in (划转入) | transfer_out (划转出) |
|------|-------------------|---------------------|
| **Pre-check** | funding balance | **user balance** |
| **PENDING 源** | funding_account | **user_trading_account** |
| **PENDING 目标** | user_trading_account | **funding_account** |
| **锁定的账户** | funding.debits_pending | **user.debits_pending** |
| **UBSCore 操作** | 用户入账 | **用户扣款** |
| **POST 完成** | user.credits_posted++ | **funding.credits_posted++** |

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
    summary: "User balance locked for transfer_out {{ $labels.request_id }}"

# User balance 锁定超过 2 小时
- alert: TransferOutUserBalanceLockCritical
  expr: transfer_out_pending_age_seconds > 7200
  labels:
    severity: critical
  annotations:
    summary: "User balance locked > 2h: {{ $labels.request_id }}"

# VOID 失败
- alert: TransferOutVoidFailed
  expr: transfer_out_void_failed_total > 0
  labels:
    severity: critical
  annotations:
    summary: "Transfer out VOID failed, user balance stuck!"
```

---

## 10. API 限流和业务控制

### 10.1 限流策略（业务优化，非安全必需）

**说明:**
- TB PENDING 机制已自然防止余额超支
- 限流主要用于业务优化和用户体验
- 不是安全必需，是合理的业务控制

```rust
// Gateway 限流配置（可选）
const TRANSFER_OUT_RATE_LIMIT: RateLimit = RateLimit {
    // 每用户每分钟最多 10 个划转请求（防止误操作）
    per_user_per_minute: 10,

    // 全局每秒最多 100 个划转请求（系统容量）
    global_per_second: 100,

    // 单笔划转最小金额（防止垃圾请求，节省资源）
    min_amount: 10_000_000,  // 10 USDT (8 decimals)
};

// 注意: 不限制 pending 数量
// 原因: TB 的 debits_pending 已自然限制
```

### 10.2 大额划转验证（可选）

```
大额划转阈值: 10,000 USDT

流程:
  1. 用户提交划转请求
  2. Gateway 检查金额
  3. 如果 amount > 10,000 USDT:
     → 标记为 "pending_verification"
     → 发送验证码到用户手机/邮箱
     → 用户输入验证码
     → 验证通过后继续正常流程
  4. 否则直接处理

优势:
  - 防止账户被盗后大额划转（用于外部提现）
  - 增加一层安全保护
  - 用户体验和安全的平衡
```

### 10.3 异常检测

```
监控异常划转行为:

1. 短时间大量划转
   - 1 小时内划转 > 10 次 → 告警

2. 金额异常
   - 单笔划转 > 用户历史平均 10 倍 → 告警

3. 新注册用户大额划转
   - 注册 < 24h 且划转 > 1000 USDT → 人工审核

4. 地理位置异常
   - IP 地址突然变化 + 大额划转 → 告警
```

---

## 11. 人工操作 SOP

### 11.1 PENDING 超过 2 小时处理流程

```
1. 查询 UBSCore 日志
   - 是否收到 request？
   - 是否已处理？
   - 处理结果？

2. 如果 UBSCore 未收到:
   → 手动 VOID
   → 释放用户余额
   → 更新 DB: failed

3. 如果 UBSCore 已扣款成功:
   → 等待 Kafka 消息
   → 或手动 POST_PENDING
   → 更新 DB: success

4. 如果 UBSCore 已拒绝:
   → 手动 VOID
   → 释放用户余额
   → 更新 DB: failed

5. 如果 UBSCore 处理中:
   → 继续等待
   → 1 小时后重新检查
```

---

**设计版本**: v1.0
**最后更新**: 2025-12-11
**基于**: Transfer In Design v1.0
**作者**: Trading System Team
