# Internal Transfer API

内部账户划转 API 定义

---

## 1. 概述

**功能:** 用户在内部账户间划转资金

**当前支持:**
- Funding ↔ Spot

**内部实现:** 参见 `FUNDING_ACCOUNT_DESIGN.md` 和 `TRANSFER_OUT_DESIGN.md`

---

## 2. 数据结构

### 2.1 账户类型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "account_type", rename_all = "snake_case")]
pub enum AccountType {
    Funding { asset: String },
    Spot { user_id: u64, asset: String },
}
```

### 2.2 转账状态

```rust
pub enum TransferStatus {
    Requesting,  // 请求中
    Pending,     // 处理中（资金已锁定）
    Success,     // 成功
    Failed,      // 失败
}
```

---

## 3. API 定义

### 3.1 创建划转

**请求:**

```http
POST /api/v1/user/internal_transfer
Authorization: Bearer {token}
Content-Type: application/json

{
  "from_account": {
    "account_type": "funding",
    "asset": "USDT"
  },
  "to_account": {
    "account_type": "spot",
    "user_id": 3001,
    "asset": "USDT"
  },
  "amount": "1000.00000000"
}
```

**请求结构:**

```rust
pub struct InternalTransferRequest {
    pub from_account: AccountType,
    pub to_account: AccountType,
    pub amount: Decimal,  // 精度由 symbol_manager 配置决定
}
```

**成功响应:**

```json
{
  "success": true,
  "data": {
    "request_id": "1734567890123456789",
    "from_account": {
      "account_type": "funding",
      "asset": "USDT"
    },
    "to_account": {
      "account_type": "spot",
      "user_id": 3001,
      "asset": "USDT"
    },
    "amount": "1000.00000000",
    "status": "pending",
    "created_at": 1702345678000
  },
  "error": null
}
```

**错误响应:**

```json
{
  "success": false,
  "data": null,
  "error": {
    "code": "INSUFFICIENT_BALANCE",
    "message": "余额不足",
    "details": {
      "available": "500.00000000",
      "required": "1000.00000000"
    }
  }
}
```

**错误码:**

| 错误码 | 说明 |
|--------|------|
| INSUFFICIENT_BALANCE | 余额不足 |
| ASSET_MISMATCH | 资产不匹配 |
| SAME_ACCOUNT | 源和目标账户相同 |
| INVALID_AMOUNT | 金额无效 |
| INVALID_PRECISION | 精度超出限制 |
| AMOUNT_TOO_SMALL | 金额低于最小限制 |
| PERMISSION_DENIED | 权限不足 |

### 3.2 查询划转状态

```http
GET /api/v1/user/internal_transfer/{request_id}
Authorization: Bearer {token}
```

**响应:** 同创建划转的响应格式

### 3.3 查询划转历史

```http
GET /api/v1/user/internal_transfer/history?limit=20&offset=0
Authorization: Bearer {token}
```

**响应:**

```json
{
  "success": true,
  "data": {
    "items": [...],
    "total": 100,
    "limit": 20,
    "offset": 0
  },
  "error": null
}
```

---

## 4. 业务规则

### 4.1 权限

- 用户只能操作自己的账户
- Funding (user_id=0) 视为系统账户，用户可以与之划转

### 4.2 验证

1. ✅ 源和目标账户的 asset 必须相同
2. ✅ 不能自己转给自己
3. ✅ 金额 > 0
4. ✅ 金额精度不超过配置限制
5. ✅ 金额 >= 最小划转金额
6. ✅ 源账户余额充足

---

## 5. 使用示例

### 5.1 充值到现货账户

```bash
curl -X POST https://api.example.com/api/v1/user/internal_transfer \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "from_account": {
      "account_type": "funding",
      "asset": "USDT"
    },
    "to_account": {
      "account_type": "spot",
      "user_id": 3001,
      "asset": "USDT"
    },
    "amount": "1000.00000000"
  }'
```

### 5.2 从现货账户划出

```bash
curl -X POST https://api.example.com/api/v1/user/internal_transfer \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json"  \
  -d '{
    "from_account": {
      "account_type": "spot",
      "user_id": 3001,
      "asset": "USDT"
    },
    "to_account": {
      "account_type": "funding",
      "asset": "USDT"
    },
    "amount": "500.00000000"
  }'
```

### 5.3 查询状态

```bash
curl https://api.example.com/api/v1/user/internal_transfer/1734567890123456789 \
  -H "Authorization: Bearer ${TOKEN}"
```

---

## 6. 注意事项

1. **request_id 唯一性:** 由服务端生成，全局唯一
2. **幂等性:** request_id 本身即为幂等性保证
3. **异步处理:** 划转请求立即返回，实际完成需要时间
4. **状态查询:** 客户端应轮询查询最终状态
5. **失败处理:** 失败时资金自动释放

---

## 7. 数据库表结构

```sql
CREATE TABLE balance_transfer_requests (
    request_id bigint PRIMARY KEY,

    -- 源账户
    from_account_type text,
    from_user_id bigint,
    from_asset_id int,

    -- 目标账户
    to_account_type text,
    to_user_id bigint,
    to_asset_id int,

    -- 金额和状态
    amount bigint,
    status text,

    -- 时间
    created_at bigint,
    updated_at bigint,

    -- 控制
    processor text,
    error_message text
);

-- 索引
CREATE INDEX idx_from_user ON balance_transfer_requests (from_user_id, created_at);
CREATE INDEX idx_to_user ON balance_transfer_requests (to_user_id, created_at);
CREATE INDEX idx_status ON balance_transfer_requests (status, created_at);
```

---

**版本**: v1.0
**更新**: 2025-12-12
**实现参考**: FUNDING_ACCOUNT_DESIGN.md, TRANSFER_OUT_DESIGN.md
