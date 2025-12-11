# Internal Transfer 实现计划 v2

**目标:** 实现内部划转功能（Funding ↔ Spot）
**原则:** 先熟悉、小步骤、独立测试、渐进集成

**修订原因:** 后端专家审核，添加准备工作，调整时间估算

---

## 📋 实施顺序

### 🔍 阶段 -1: 准备工作 (必须)

**目的:** 熟悉现有代码，确认依赖就绪

- [ ] **Step -1.1:** 代码结构分析 ⏱️ 1h
- [ ] **Step -1.2:** 依赖检查（TB, DB, Aeron, Kafka）⏱️ 30min
- [ ] **Step -1.3:** 测试环境准备 ⏱️ 30min

### 🏗️ 阶段 0: 基础设施

- [ ] **Step 0.1:** 数据结构定义 ⏱️ 1.5h
- [ ] **Step 0.2:** DB 表结构创建 ⏱️ 1h
- [ ] **Step 0.3:** DB 访问层 ⏱️ 2h
- [ ] **Step 0.4:** TigerBeetle 集成确认 ⏱️ 1h
- [ ] **Step 0.5:** 消息系统确认 ⏱️ 1h

### 🌐 阶段 1: API 层

- [ ] **Step 1.1:** API 请求/响应结构体 ⏱️ 1h
- [ ] **Step 1.2:** 验证逻辑 ⏱️ 1.5h
- [ ] **Step 1.3:** API 路由和基本框架 ⏱️ 1h

### 🔄 阶段 2: Transfer IN 核心逻辑

- [ ] **Step 2.1:** request_id 生成和持久化 ⏱️ 1.5h
- [ ] **Step 2.2:** TigerBeetle 余额查询 ⏱️ 1h
- [ ] **Step 2.3:** TigerBeetle CREATE_PENDING ⏱️ 2h
- [ ] **Step 2.4:** 状态更新逻辑 ⏱️ 1h
- [ ] **Step 2.5:** Aeron 消息发送 ⏱️ 1.5h

### 🔄 阶段 3: Transfer OUT 核心逻辑

- [ ] **Step 3.1:** Transfer OUT 特定验证 ⏱️ 1h
- [ ] **Step 3.2:** Spot 余额检查 ⏱️ 1h
- [ ] **Step 3.3:** Transfer OUT 完整流程 ⏱️ 2h

### 🔗 阶段 4: Settlement 集成

- [ ] **Step 4.1:** Kafka 消息接收 ⏱️ 1.5h
- [ ] **Step 4.2:** POST_PENDING 逻辑 ⏱️ 2h
- [ ] **Step 4.3:** 状态扫描和恢复 ⏱️ 2h

### ⚡ 阶段 5: 容错和恢复

- [ ] **Step 5.1:** Gateway 错误处理和 VOID ⏱️ 2h
- [ ] **Step 5.2:** Settlement 扫描机制 ⏱️ 2h
- [ ] **Step 5.3:** Crash 恢复测试 ⏱️ 2h

### 🧪 阶段 6: 集成测试

- [ ] **Step 6.1:** Transfer IN 端到端测试 ⏱️ 2h
- [ ] **Step 6.2:** Transfer OUT 端到端测试 ⏱️ 2h
- [ ] **Step 6.3:** 异常场景测试 ⏱️ 2h

**总计:** ~35 小时（~5 个工作日）

---

## 🎯 Step -1.1: 代码结构分析

**目的:** 熟悉现有代码，确定实现位置

### 任务清单:

1. **查看项目整体结构**
   ```bash
   tree -L 3 src/
   ls -la schema/
   ```

2. **查找类似功能作为参考**
   - 查找 deposit/withdraw 相关代码
   - 查找 balance 相关类型定义
   - 查找 API 路由定义方式

3. **确定模块位置**
   - 类型定义应该放在哪里？
   - DB schema 文件命名规范？
   - API handler 组织方式？
   - 测试文件位置？

4. **确定命名规范**
   - 文件命名风格
   - 结构体命名规范
   - 函数命名规范
   - 测试命名规范

5. **查看现有测试**
   - 运行现有测试套件
   - 查看测试覆盖率
   - 了解测试工具和框架

### 输出文档:

创建 `docs/INTERNAL_TRANSFER_CODE_STRUCTURE.md`:
```markdown
# 代码结构分析

## 项目结构
- src/types/: 类型定义
- src/db/: 数据库访问层
- src/api/: API handlers
- ...

## 参考实现
- deposit 功能位于: xxx
- 类似的类型定义: xxx

## 命名规范
- 文件: snake_case.rs
- 结构体: PascalCase
- 函数: snake_case

## 测试策略
- 单元测试: 使用 mockall
- 集成测试: tests/ 目录
- 覆盖率工具: tarpaulin
```

### 验收标准:

- ✅ 了解项目目录结构
- ✅ 找到至少一个类似功能作为参考
- ✅ 确定所有新文件的位置
- ✅ 了解测试策略
- ✅ 输出文档完整

### 预估时间: 1 小时

---

## 🎯 Step -1.2: 依赖检查

**目的:** 确认所有外部依赖就绪

### 任务清单:

1. **TigerBeetle 客户端检查**
   ```rust
   // 找到 TB 客户端初始化代码
   // 确认账户创建方式
   // 查看现有 TB 操作示例
   ```

2. **ScyllaDB 连接检查**
   ```rust
   // 确认 DB session 获取方式
   // 查看现有 schema
   // 了解 migration 流程
   ```

3. **Aeron 配置检查**
   ```rust
   // 找到 Aeron 发送代码
   // 确认消息格式
   // 了解 UBSCore 通信方式
   ```

4. **Kafka 配置检查**
   ```rust
   // 找到 Kafka producer/consumer
   // 确认 topic 命名
   // 了解消息序列化方式
   ```

5. **Symbol Manager 集成**
   ```rust
   // 找到 symbol_manager 使用方式
   // 确认精度验证接口
   ```

### 输出清单:

创建检查清单 `docs/INTERNAL_TRANSFER_DEPENDENCIES_CHECK.md`:
```markdown
# 依赖检查清单

## TigerBeetle
- [x] 客户端已集成: src/xxx
- [x] Funding 账户创建方式: xxx
- [x] Spot 账户创建方式: xxx
- [x] 现有操作示例: src/xxx

## ScyllaDB
- [x] Session 初始化: src/xxx
- [x] Schema 位置: schema/xxx
- [x] Migration 工具: xxx

## Aeron
- [x] 发送客户端: src/xxx
- [x] 消息格式: proto/xxx 或 sbe/xxx
- [x] UBSCore topic: xxx

## Kafka
- [x] Producer: src/xxx
- [x] Consumer: src/xxx
- [x] Topic 命名: settlement_xxx

## Symbol Manager
- [x] 使用示例: src/xxx
- [x] 精度验证: SYMBOL_MANAGER.get_symbol()
```

### 验收标准:

- ✅ 所有依赖状态明确
- ✅ 找到所有依赖的使用示例
- ✅ 确认依赖可用
- ✅ 输出清单完整

### 预估时间: 30 分钟

---

## 🎯 Step -1.3: 测试环境准备

**目的:** 准备测试工具和策略

### 任务清单:

1. **确定 Mock 框架**
   ```toml
   # Cargo.toml
   [dev-dependencies]
   mockall = "0.12"  # 或项目使用的其他框架
   ```

2. **准备测试数据 Fixtures**
   ```rust
   // tests/fixtures/internal_transfer.rs
   pub fn sample_funding_account() -> AccountType { ... }
   pub fn sample_spot_account() -> AccountType { ... }
   pub fn sample_transfer_request() -> InternalTransferRequest { ... }
   ```

3. **配置测试覆盖率**
   ```bash
   cargo install cargo-tarpaulin
   cargo tarpaulin --out Html
   ```

4. **准备测试数据库**
   ```bash
   # 本地 ScyllaDB for testing
   docker-compose up -d scylla-test
   ```

5. **CI/CD 检查**
   - 查看现有 CI 配置
   - 确认测试在 CI 中如何运行

### 输出配置:

创建测试配置 `docs/INTERNAL_TRANSFER_TEST_STRATEGY.md`:
```markdown
# 测试策略

## Mock 框架
- 主框架: mockall
- 异步 mock: tokio-test

## 测试数据
- Fixtures 位置: tests/fixtures/
- 测试账户: user_id = 9999 (test)

## 覆盖率要求
- 最低覆盖率: 80%
- 核心逻辑: 100%
- 工具: cargo-tarpaulin

## 测试分类
- 单元测试: #[test]
- 异步测试: #[tokio::test]
- 集成测试: tests/ 目录

## 测试隔离
- 每个测试独立
- 使用不同的 request_id
- Mock 外部依赖
```

### 验收标准:

- ✅ Mock 框架已添加到依赖
- ✅ Fixtures 模块已创建
- ✅ 测试覆盖率工具可用
- ✅ 测试数据库准备就绪
- ✅ 测试策略文档完整

### 预估时间: 30 分钟

---

## 🔄 实施流程（每个 Step）

### 1. Review 设计文档 (5 min)
- 重新阅读对应章节
- 确认实现细节
- 记录关键点

### 2. 编写代码 (主要时间)
- 小步骤实现
- 专注单一功能
- 及时提交暂存

### 3. 编写测试 (与代码同步)
- 测试驱动或并行
- 覆盖正常和异常路径
- 确保测试可通过

### 4. 自我 Review (10 min)
- 检查与设计文档一致性
- 检查代码质量
- 检查测试完整性

### 5. 运行测试 (5 min)
```bash
cargo test --lib <module>
cargo tarpaulin --packages <crate>
```

### 6. Commit (5 min)
```bash
git add <files>
git commit -m "feat: <step description>

- 完成 Step X.Y
- 实现 <feature>
- 测试覆盖率 X%
"
```

### 7. 更新进度 (5 min)
- 在本文档中标记完成 ✅
- 记录遇到的问题
- 记录偏差和调整

---

## 📊 时间规划总览

| 阶段 | 步骤数 | 预估时间 |
|------|--------|---------|
| 阶段 -1: 准备 | 3 | 2h |
| 阶段 0: 基础 | 5 | 6.5h |
| 阶段 1: API | 3 | 3.5h |
| 阶段 2: Transfer IN | 5 | 7h |
| 阶段 3: Transfer OUT | 3 | 4h |
| 阶段 4: Settlement | 3 | 5.5h |
| 阶段 5: 容错 | 3 | 6h |
| 阶段 6: 集成测试 | 3 | 6h |
| **总计** | **28** | **~40h** |

**实际工作日:** 5-7 天（考虑 Code Review, 问题排查等）

---

## ✅ 准备开始

**当前状态:** 📝 待开始
**下一步:** Step -1.1 代码结构分析

**准备好了吗？** 👍
