# Funding Account & Transfer Design

完整的 Gateway 资金池（Funding Account）与用户账户（User Trading Account）之间的**内部划转**设计文档。

**重要说明:**
- `transfer_in`: 内部划转，从 funding_account → user_trading_account
- `transfer_out`: 内部划转，从 user_trading_account → funding_account
- 这是交易所内部的账户管理，用于不同用途账户之间的资金划转
- **不是外部充值/提现**
- 外部操作使用 `deposit` (充值) / `withdraw` (提现) 术语

---

## 📋 目录

- [1. 总体架构](#1-总体架构)
- [2. 账户体系](#2-账户体系)
- [3. 状态机设计](#3-状态机设计)
- [4. 数据流](#4-数据流)
- [5. 超时策略](#5-超时策略)
- [6. 容错机制](#6-容错机制)
- [7. 实现细节](#7-实现细节)

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
│  - 发起 transfer_in/transfer_out 请求                       │
│  - 轮询查询状态                                              │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│                        Gateway                              │
│  - Pre-check funding balance                                │
│  - 创建 TB PENDING (锁定资金)                                │
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
│  - 内部入账逻辑（UBSCore 责任）                              │
│  - 发送明确响应（成功/失败/错误）                            │
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
│  - 清理孤儿记录 (24小时)                                     │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 职责边界

**Gateway:**
- ✅ 锁定资金（CREATE_PENDING）
- ✅ 发送请求到 UBSCore
- ✅ 处理 UBSCore 明确响应
- ✅ 失败时 VOID（先 VOID，再更新 DB）
- ❌ 不关心 UBSCore 内部入账逻辑

**UBSCore:**
- ✅ 接收并处理 transfer request
- ✅ 内部入账逻辑（UBSCore 负责）
- ✅ 返回明确响应（成功/拒绝/错误）
- ✅ 成功后发送确认到 Settlement
- ❌ Caller 无需知道内部实现

**Settlement:**
- ✅ 接收 UBSCore 确认
- ✅ POST_PENDING 完成流转
- ✅ 扫描和恢复中间状态
- ✅ 从 TB 同步状态
- ❌ 永不自动 VOID

---

## 2. 账户体系

### 2.1 TigerBeetle 账户

```rust
// Funding Account (Gateway 资金池)
let funding_account_id = tb_account_id(FUNDING_USER_ID, asset_id);
// FUNDING_USER_ID = 0
// 用途: Gateway 的充值资金池
// 操作者: Gateway (CREATE_PENDING), Settlement (POST_PENDING)

// User Trading Account (用户交易账户)
let user_account_id = tb_account_id(user_id, asset_id);
// 用途: 用户在交易系统内的余额
// 操作者: UBSCore (入账), Settlement (POST_PENDING)
```

### 2.2 账户独立性

- **Funding Account** 和 **User Trading Account** 完全独立
- 通过 TB Transfer 连接: `funding → user`
- Gateway 只操作 funding account
- UBSCore 只操作 user trading account
- Settlement 协调两者的资金流转

---

## 3. 状态机设计

### 3.1 状态定义

```rust
pub enum TransferStatus {
    /// Gateway 已持久化请求，预检查通过
    Requesting,

    /// 资金已锁定（TB PENDING created）
    Pending,

    /// 完成（UBSCore 入账 + Settlement POST 成功）
    Success,

    /// 失败（明确拒绝或超时 VOID）
    Failed,
}
```

### 3.2 状态优先级

```
requesting (0) → pending (1) → success/failed (2)
                            ↘
```

**规则:**
- ✅ 只能前进（优先级递增）
- ❌ 不能后退
- ✅ 终态（success/failed）不能转换

### 3.3 允许的状态转换

```
requesting → pending    (Gateway 锁定 / Settlement 恢复)
requesting → success    (Settlement 恢复，极端情况)
requesting → failed     (Gateway 拒绝 / Settlement 清理)

pending → success       (Settlement POST 成功)
pending → failed        (Settlement 超时 VOID / Gateway 拒绝)

success → X             (终态，不允许)
failed → X              (终态，不允许)
```

### 3.4 转换原因

```rust
pub enum TransitionReason {
    GatewayLocked,                      // Gateway 创建 TB PENDING
    GatewayRejected(String),            // Gateway 明确拒绝
    GatewayTbError(String),             // Gateway TB 失败
    UbscoreConfirmed,                   // UBSCore 确认
    SettlementPosted,                   // Settlement POST 成功
    SettlementTimeout,                  // Settlement 超时 VOID
    SettlementRecovered(String),        // Settlement 从 TB 恢复
    SettlementCleanup,                  // Settlement 清理孤儿
}
```

---

## 4. request_id 生成策略

### 4.1 使用 SnowflakeGen with RandomSequence

**要求:**
- ✅ 全局唯一（绝对无碰撞）
- ✅ 时间有序（便于查询和范围扫描）
- ✅ 高性能（无需网络协调）
- ✅ u64 类型（兼容 TB transfer.id）
- ✅ 单调递增（同一实例内）

**方案:**

```rust
type RequestIdGenerator = SnowflakeGen<RandomSequence>;

// 结构:
// [41 bits timestamp] [10 bits machine_id] [13 bits sequence]
//
// - 时间戳: 精确到毫秒
// - 机器 ID: 支持 1024 个实例
// - 序列号: 0-8191，单调递增
```

**关键特性:**
- ✅ **绝对唯一**: timestamp + machine_id + sequence 三元组唯一
- ✅ **单调递增**: 同一实例生成的 ID 单调递增
- ✅ **时间有序**: 支持高效范围查询
- ✅ **高吞吐**: 单机每秒 800 万+ ID
- ✅ **零碰撞**: 内部实现保证唯一性（包括时间回拨处理）

**容量:**
```
单机:
  - 每毫秒: 8192 个 ID
  - 每秒: 8,192,000 个 ID
  - 实际需求: ~1000 请求/秒
  - 容量富余: 8000 倍

多实例:
  - 1024 个实例理论容量: 83 亿 ID/秒
```

**职责分配:**
- ✅ **request_id 唯一性** → SnowflakeGen 保证
- ✅ **防止双重处理** → 具体处理方（UBSCore/Settlement）基于 request_id 去重
- ✅ **幂等性** → UBSCore dedup + Settlement 状态检查

### 4.2 生成位置

```rust
// Gateway AppState
struct AppState {
    request_id_gen: Arc<Mutex<SnowflakeGen<RandomSequence>>>,
    // ...
}

// Gateway
let request_id = {
    let mut gen = state.request_id_gen.lock().unwrap();
    gen.generate()  // SnowflakeGen<RandomSequence>::generate() -> u64
};
```

### 4.3 Dedup 机制

**UBSCore Dedup:**
```rust
// 检查是否已处理
if processed_cache.contains(&request_id) {
    tracing::info!("Duplicate request: {}", request_id);
    return cached_result;
}

// 处理
process_transfer_in(request_id, ...);

// 缓存结果 (TTL: 1 hour)
processed_cache.insert(request_id, result, 3600);
```

**Settlement Dedup:**
```rust
// 状态检查
let current = db.get_transfer_request(request_id).await?;

if current.status == "success" {
    tracing::info!("Already succeeded: {}", request_id);
    return Ok(());
}

// 继续处理
```

---

## 5. 数据流

### 4.1 transfer_in (充值) - 正常流程

```
T0: Client → Gateway
    POST /api/v1/user/transfer_in
    { user_id, asset, amount }

T0: Gateway
    1. Pre-check funding_account balance (TB)
       └─ available >= amount? 否则 503
       └─ **目的**: 快速失败，减少无效 request 生成
       └─ **注意**: 非原子操作，只是提示性检查
       └─ **真正防超卖**: 依赖 TB CREATE_PENDING 的原子性
       └─ **并发控制**: 未来通过 API Rate Limit (Gateway 责任)
    2. Insert ScyllaDB
       └─ status: "requesting"
       └─ **关键**: 必须先持久化，才能 lock funding
       └─ **原因**: 防止 crash 后丢失 request 记录
       └─ **恢复**: Settlement 可从 DB 查到，再查 TB 恢复状态

    3. TB CREATE_PENDING (id: request_id)
       funding_account → user_trading_account
       └─ funding.debits_pending += amount 🔒

    4. Update ScyllaDB
       └─ status: "pending"

    5. Aeron send to UBSCore (retry 3次)
       └─ 网络错误 → 重试
       └─ 明确拒绝 → VOID + failed
       └─ 未知错误 → 保持 pending

    6. Return response
       └─ { request_id }

T0-T20s: Gateway 重试 Aeron (最多 20秒)

T20s: Gateway 超时退出
      └─ status 保持 "pending"

UBSCore:
    7. 收到 Aeron 消息 (Dedup: request_id)

    8. 用户入账 (TB)
       └─ user_trading_account.balance += amount

    9. Kafka publish to Settlement
       └─ BalanceUpdateEvent

Settlement (Kafka path):
    10. 收到 BalanceUpdateEvent

    11. TB POST_PENDING (无限重试)
        └─ POST transfer (pending_id: request_id)
        └─ funding.debits_posted += amount
        └─ funding.debits_pending -= amount 🔓
        └─ user.credits_posted += amount

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
  - TB: PENDING exists (资金已锁定)

Settlement 扫描 (T30s 后):
  1. 查询 ScyllaDB: status = "requesting"

  2. 查询 TB: lookup_transfers([request_id])
     └─ 找到 PENDING

  3. 恢复状态:
     └─ requesting → pending ✅

  4. 继续正常流程
     └─ 等待 UBSCore 或超时
```

---

## 5. 超时策略

### 5.1 超时常量

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
T30min     PENDING 告警（⚠️ 等待超时，但不 VOID）
T2h        PENDING 严重告警（🚨 需人工核查）
T24h       requesting 清理 → failed (✅ 无资金锁定)
```

### 5.3 PENDING 状态处理策略

**关键安全原则: 永不自动 VOID**

```
PENDING 状态的唯一两种结束方式:

1. ✅ UBSCore 发送成功确认
   → Settlement POST_PENDING → success

2. ✅ 人工确认失败
   → 管理员手动 VOID → failed

禁止:
❌ Settlement 自动 VOID
❌ 超时自动 VOID
❌ 任何自动 VOID
```

**原因:**
```
场景: Settlement 自动 VOID
时间线:
  T1: Gateway CREATE_PENDING (funding 锁定 1000)
  T2: Aeron 发送成功,但 UBSCore 处理慢
  T30min: Settlement 超时自动 VOID (funding 释放)
  T31min: UBSCore 处理完成，用户入账 +1000

结果:
  - 用户: +1000 ✅
  - Funding: 已释放 ✅
  - 总账: 凭空产生 1000 💸

风险: 资金灾难
```

### 5.4 超时告警策略

| 状态 | 阈值 | 动作 | 原因 |
|------|------|------|------|
| `requesting` | 24 小时 | 清理 → failed | 无资金锁定，安全 |
| `pending` | 30 分钟 | ⚠️ 告警 | 等待超时，但不 VOID |
| `pending` | 2 小时 | 🚨 严重告警 | 需人工核查 |
| `pending` | 24 小时 | 🚨🚨 紧急告警 | 需立即人工介入 |

**告警内容:**
```
30 分钟告警:
  "PENDING request {} 等待超过 30min，请查询 UBSCore 状态"

2 小时严重告警:
  "PENDING request {} 等待超过 2h，请立即人工核查！
   1. 查询 UBSCore 是否已处理
   2. 如确认失败，手动执行 VOID
   3. 如确认成功，等待 UBSCore 消息或手动 POST"

24 小时紧急告警:
  "PENDING request {} 卡住超过 24h！
   资金已锁定 {} {}！
   立即人工介入！"
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

2. 明确业务拒绝 (USER_NOT_FOUND, INSUFFICIENT_BALANCE)
   → VOID TB PENDING
   → 更新 status: "failed"
   → 返回错误

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
     └─ age >= 24h: 清理 → failed (无资金锁定，安全)

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
     └─ 检查 funding 和 user 账户状态
     └─ 根据账户状态同步 DB:
        - 如果 user.credits_posted 增加 → success
        - 如果无变化 → 可能已 VOID
     └─ 如果仍无法确定 → 告警，等待人工核查

### 6.3 Gateway 明确拒绝时的 VOID

**唯一允许 Gateway 自动 VOID 的场景:**

```rust
// UBSCore 明确的业务拒绝
if let Some(business_error) = parse_business_error(&aeron_error) {
    match business_error.reason.as_str() {
        "USER_NOT_FOUND" | "INVALID_ASSET" => {
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

                    tracing::info!("✅ VOIDED rejected request: {}", request_id);
                }
                Err(e) => {
                    // ❌ VOID 失败
                    tracing::error!(
                        "🚨 VOID failed for {}: {}. TB PENDING stuck!",
                        request_id, e
                    );

                    alert_critical(format!(
                        "VOID failed for rejected request {}: {}. \
                         TB PENDING exists but cannot release!",
                        request_id, e
                    ));

                    // 不更新 DB 状态，保持 pending
                    // 让 Settlement 扫描处理
                    // Settlement 会查询 TB，发现仍是 PENDING
                    // 等待人工介入
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
2. ✅ VOID 成功 → 更新 DB status = failed
3. ❌ VOID 失败 → 不更新 DB，保持 pending
4. ✅ Settlement 扫描会处理卡住的情况

### 6.4 VOID 决策表（修正）

| 场景 | 可否 VOID | 原因 |
|------|----------|------|
| UBSCore 明确拒绝 (USER_NOT_FOUND) | ✅ Yes (Gateway) | 确认未入账 |
| UBSCore 明确拒绝 (INVALID_ASSET) | ✅ Yes (Gateway) | 确认未入账 |
| PENDING 超时 (任何时长) | ❌ No | 不知道 UBSCore 状态 |
| Aeron 网络错误 | ❌ No | 可能已发送 |
| Aeron 未知错误 | ❌ No | 不确定状态 |
| UBSCore 错误 | ❌ No | 可能已入账 |
| 人工确认失败 | ✅ Yes (手动) | 人工核查后确认 |

### 6.5 CAS 失败处理

**问题**: ScyllaDB CAS 更新可能失败，原因不明确

**解决方案**:
```rust
match db.transition_transfer_status(request_id, from, to, reason, ...).await {
    Ok(true) => {
        // ✅ CAS 成功
        tracing::info!("Status updated: {} → {}", from, to);
    }
    Ok(false) => {
        // ⚠️ CAS 失败，查询当前状态
        let current = db.get_transfer_request(request_id).await?;

        if current.status == to.as_str() {
            // 幂等：已是目标状态
            tracing::info!("Already in state: {}", to);
            return Ok(());
        } else if TransferStatus::from_str(&current.status)?.priority() > to.priority() {
            // 已前进到更高优先级状态
            tracing::info!(
                "Status already advanced: expected {}, found {}",
                from, current.status
            );
            return Ok(());
        } else {
            // 状态不符合预期
            alert_critical(format!(
                "CAS failed with unexpected state: request={}, \
                 expected {}, found {}, want {}",
                request_id, from, current.status, to
            ));
            return Err("CAS conflict");
        }
    }
    Err(e) => {
        // ❌ DB 错误
        tracing::error!("DB error for request {}: {}", request_id, e);
        alert_critical(format!("DB error: {}", e));
        return Err(e);
    }
}
```

### 6.6 POST_PENDING 永久失败监控

**问题**: Settlement POST_PENDING 可能永久失败（TB 问题）

**解决方案**:
```rust
// Settlement 监控
async fn monitor_stuck_post_pending() {
    loop {
        sleep(Duration::from_secs(300)).await;  // 每 5 分钟

        // 查询重试次数超过阈值的请求
        let stuck = db.get_requests_by_retry_count(1000).await?;

        for req in stuck {
            alert_critical(format!(
                "POST_PENDING stuck: request={}, retries={}, \
                 UBSCore已入账但Settlement无法完成！需人工介入！",
                req.request_id, req.retry_count
            ));

            // 标记为需要人工处理（但保持 pending 状态）
            db.update_error_message(
                req.request_id,
                format!("POST重试{}次失败,需人工介入", req.retry_count)
            ).await.ok();
        }
    }
}

// PENDING 告警（每 5 分钟）
async fn alert_long_pending() {
    loop {
        sleep(Duration::from_secs(300)).await;

        let now = current_time_ms();
        let pending = db.get_transfer_requests_by_status("pending").await?;

        for req in pending {
            let age = now - req.created_at;

            if age > 24 * 3600 * 1000 {  // 24 小时
                alert_critical(format!(
                    "🚨🚨 PENDING {} 超过 24h！资金锁定 {} {}！立即人工介入！",
                    req.request_id, req.amount, req.asset_id
                ));
            } else if age > 2 * 3600 * 1000 {  // 2 小时
                alert_ops_team(AlertSeverity::Critical, format!(
                    "🚨 PENDING {} 超过 2h，需人工核查 UBSCore 状态",
                    req.request_id
                ));
            } else if age > 30 * 60 * 1000 {  // 30 分钟
                alert_ops_team(AlertSeverity::Warning, format!(
                    "⚠️ PENDING {} 超过 30min，请查询 UBSCore 状态",
                    req.request_id
                ));
            }
        }
    }
}
```

**注意**:
- 资金对账（Reconciliation）是独立的系统级功能
- 超出本设计文档范围
- 将在专门的对账设计文档中详细定义

---

## 7. 实现细节

### 7.1 ScyllaDB 表结构

```sql
CREATE TABLE balance_transfer_requests (
    request_id bigint PRIMARY KEY,
    user_id bigint,
    asset_id int,
    amount bigint,
    direction text,                 -- 'in' or 'out'
    status text,                    -- 'requesting', 'pending', 'success', 'failed'
    pending_transfer_id bigint,     -- TB PENDING transfer id
    posted_transfer_id bigint,      -- TB POST transfer id (if posted)
    created_at bigint,
    updated_at bigint,
    last_retry_at bigint,
    retry_count int,
    processor text,                 -- 'gateway' or 'settlement'
    error_message text
);

CREATE INDEX ON balance_transfer_requests (status);
CREATE INDEX ON balance_transfer_requests (created_at);
CREATE INDEX ON balance_transfer_requests (user_id, created_at);
```

### 7.2 状态更新 CAS

```rust
// 使用 Compare-And-Set 保证原子性
UPDATE balance_transfer_requests
SET status = ?,
    posted_transfer_id = ?,
    error_message = ?,
    updated_at = ?
WHERE request_id = ?
  AND status = ?  -- CAS: 只在当前状态匹配时更新
```

### 7.3 状态机验证

```rust
// 所有状态更新都通过状态机验证
let result = TransferStateMachine::validate_transition(
    TransferStatus::Requesting,
    TransferStatus::Pending,
    &TransitionReason::GatewayLocked,
);

match result {
    TransitionResult::Allowed => {
        // 执行 CAS 更新
    }
    TransitionResult::Rejected(reason) => {
        // 拒绝，记录日志
    }
    TransitionResult::AlreadyInState => {
        // 幂等，直接返回成功
    }
}
```

### 7.4 TB Two-Phase Transfer

```
Phase 1: CREATE_PENDING
  Transfer {
    id: request_id,
    debit_account_id: funding_account,
    credit_account_id: user_account,
    amount: raw_amount,
    flags: PENDING,  // 🔒
  }

  结果:
    funding.debits_pending += amount
    user.credits_pending += amount (不可用)

Phase 2: POST_PENDING
  Transfer {
    id: new_id,
    pending_id: request_id,  // 🔗 引用 PENDING
    flags: POST_PENDING_TRANSFER,  // ✅
  }

  结果:
    funding.debits_posted += amount
    funding.debits_pending -= amount
    user.credits_posted += amount
    user.credits_pending -= amount
    → 用户可用余额增加 ✅

Phase 2: VOID_PENDING (失败时)
  Transfer {
    id: new_id,
    pending_id: request_id,
    flags: VOID_PENDING_TRANSFER,  // ❌
  }

  结果:
    funding.debits_pending -= amount  // 🔓 释放
    user.credits_pending -= amount
    → 资金返回 funding 可用余额
```

---

## 8. API 接口

### 8.1 发起充值

```http
POST /api/v1/user/transfer_in
Content-Type: application/json

{
  "user_id": 3001,
  "asset": "USDT",
  "amount": "1000.00"
}

Response:
{
  "success": true,
  "message": "Request 1234567890 submitted",
  "request_id": "1234567890"
}
```

### 8.2 查询状态

```http
GET /api/v1/transfer/status/1234567890

Response:
{
  "request_id": "1234567890",
  "status": "success",
  "user_id": 3001,
  "asset_id": 1,
  "amount": 1000000000,
  "created_at": 1702345678000,
  "updated_at": 1702345680000
}
```

### 8.3 查询历史

```http
GET /api/v1/user/3001/transfers/recent?direction=in&limit=20

Response:
{
  "user_id": 3001,
  "direction": "in",
  "total": 5,
  "items": [
    {
      "request_id": "1234567890",
      "asset": "USDT",
      "amount": "1000.00",
      "status": "success",
      "direction": "in",
      "created_at": 1702345678000,
      "updated_at": 1702345680000
    }
  ]
}
```

---

## 9. 监控告警

### 9.1 关键指标

```
- transfer_requests_total{status, direction}
- transfer_pending_age_seconds
- transfer_requesting_age_seconds
- gateway_aeron_errors_total{type}
- settlement_void_total{reason}
- settlement_scan_duration_seconds
- tb_pending_count
```

### 9.2 告警规则

```yaml
# PENDING 超过 10 分钟
- alert: TransferPendingTooLong
  expr: transfer_pending_age_seconds > 600
  severity: warning

# requesting 超过 1 小时
- alert: TransferRequestingTooLong
  expr: transfer_requesting_age_seconds > 3600
  severity: warning

# VOID 失败
- alert: VoidTransferFailed
  expr: settlement_void_errors_total > 0
  severity: critical
```

---

## 10. 测试策略

### 10.1 单元测试

- 状态机转换规则
- 状态优先级
- CAS 更新逻辑

### 10.2 集成测试

- Gateway → UBSCore → Settlement 完整流程
- Gateway crash 恢复
- Settlement 扫描恢复
- 超时 VOID

### 10.3 混沌测试

- 随机 crash Gateway
- 随机网络延迟/丢包
- TB/ScyllaDB 故障注入
- 并发请求压测

---

## 11. 安全保证

### 11.1 数据一致性

✅ TB 是真相源
✅ ScyllaDB 状态可从 TB 重建
✅ 状态只能前进（CAS + 状态机）
✅ 幂等性（request_id + UBSCore dedup）

### 11.2 资金安全

✅ PENDING 锁定资金（防止超卖）
✅ 只在明确失败时 VOID
✅ POST 无限重试（确保完成）
✅ 超时 VOID 释放锁定资金

### 11.3 容错能力

✅ Gateway crash → Settlement 扫描恢复
✅ Settlement crash → 重启后继续处理
✅ 网络错误 → 安全重试
✅ 未知错误 → 保守处理

---

## 12. 性能预估

```
内网延迟:
  - TB operation:        ~1ms
  - Aeron send:          ~0.5ms
  - Kafka latency:       ~5ms
  - ScyllaDB write:      ~3ms

正常流程耗时:
  - Gateway 处理:        ~10ms
  - UBSCore 处理:        ~10ms
  - Settlement 处理:     ~10ms
  Total:                 ~30ms (P50)
                         ~100ms (P99)

Client 轮询:
  - 首次查询:            100ms 后
  - 间隔:                100ms
  - 预期确认时间:        200-500ms
```

---

## 13. 未来优化

1. **异步确认机制**
   - WebSocket 推送状态变化
   - 避免客户端轮询

2. **批量处理**
   - Settlement 批量 POST_PENDING
   - 提高吞吐量

3. **状态缓存**
   - Redis 缓存状态
   - 减少 ScyllaDB 查询

4. **动态超时**
   - 根据系统负载调整
   - P99 延迟自适应

---

**设计版本**: v1.0
**最后更新**: 2025-12-11
**作者**: Trading System Team
