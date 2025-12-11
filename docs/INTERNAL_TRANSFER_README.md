# Internal Transfer 文档说明

## 📚 文档结构

### ✅ 新版文档（推荐使用）

使用 **Funding** 和 **Spot** 账户类型：

```
├── INTERNAL_TRANSFER_API.md          # API 定义（接口层）
├── INTERNAL_TRANSFER_IN_IMPL.md      # Transfer IN 实现（Funding → Spot）
└── INTERNAL_TRANSFER_OUT_IMPL.md     # Transfer OUT 实现（Spot → Funding）
```

**特点:**
- ✅ 账户名称简洁：`Funding`, `Spot`
- ✅ 统一命名规范：`INTERNAL_TRANSFER_*`
- ✅ API 与实现分离
- ✅ 完整的设计原则和容错机制

### 📖 旧版文档（保留作为参考）

使用 **funding_account** 和 **user_trading_account**：

```
├── FUNDING_ACCOUNT_DESIGN.md         # 旧版 transfer_in 设计
└── TRANSFER_OUT_DESIGN.md            # 旧版 transfer_out 设计
```

**特点:**
- 经过严格审核的设计
- 使用更完整的账户名称
- 包含详细的实现细节

---

## 🎯 使用建议

### 新项目/重构
**推荐使用新版文档**
- 更简洁的命名
- 更清晰的结构
- API 层定义明确

### 维护现有代码
**可参考旧版文档**
- 如果现有代码使用 `user_trading_account`
- 需要详细的实现参考

---

## 📊 账户类型对比

| 旧版 | 新版 | 说明 |
|------|------|------|
| `funding_account` | `Funding` | 资金池账户（user_id=0）|
| `user_trading_account` | `Spot` | 用户现货账户 |

**核心逻辑完全相同**，只是命名不同。

---

## 🔄 迁移路径

如果要从旧版迁移到新版：

```rust
// 旧代码
let user_trading_account_id = tb_account_id(user_id, asset_id);

// 新代码
let spot_account_id = tb_account_id(user_id, asset_id);
```

只需要重命名变量，逻辑不变。

---

**更新日期**: 2025-12-12
**状态**: 新旧版本并存，推荐使用新版
