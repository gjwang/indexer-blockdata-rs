# LEDGER ARCHITECTURE REVIEW & INTEGRATION PLAN

**Date:** 2025-12-10
**Context:** UBSCore currently uses simple HashMap for account storage. Battle-tested GlobalLedger exists but isn't integrated.

---

## 🔍 CURRENT STATE

### 1. GlobalLedger (`src/ledger.rs`)
**Status:** Battle-tested, feature-complete, NOT USED in production

**Features:**
- ✅ Write-Ahead Logging (WAL) with rolling segments
- ✅ Snapshot support for fast recovery
- ✅ MD5 verification for data integrity
- ✅ Full replay capability
- ✅ Account balance tracking with versions
- ✅ Comprehensive test coverage (1854 lines total)

**Key Components:**
```rust
pub struct GlobalLedger {
    accounts: HashMap<UserId, UserAccount>,
    wal: RollingWal,
    snapshot_manager: SnapshotManager,
    sequence: u64,
    ledger_dir: PathBuf,
    snapshot_dir: PathBuf,
}
```

**Methods:**
- `deposit()` - Add funds with WAL logging
- `withdraw()` - Remove funds with WAL logging
- `apply()` - Apply LedgerCommand with persistence
- `snapshot()` - Create snapshot for recovery
- `replay()` - Replay WAL from sequence number

### 2. UBSCore (`src/ubs_core/core.rs`)
**Status:** Currently in use, lightweight, NO persistence

**Current Storage:**
```rust
pub struct UBSCore<R: RiskModel> {
    accounts: HashMap<UserId, UserAccount>,  // ← Simple HashMap, no WAL
    dedup_guard: DeduplicationGuard,
    debt_ledger: DebtLedger,
    // ...
    wal: Option<GroupCommitWal>,  // ← Different WAL (for orders only)
}
```

**Current Implementation:**
- Uses simple HashMap directly
- Has its own WAL (`GroupCommitWal`) for order deduplication
- No balance WAL/snapshot integration
- Balances exist only in memory

---

## 🎯 INTEGRATION GOALS

### Primary Objective
**Replace UBSCore's HashMap with GlobalLedger** to get:
1. **Durability** - Balance changes persisted to WAL
2. **Recovery** - Fast startup from snapshots
3. **Audit Trail** - Complete history of balance changes
4. **Integrity** - MD5 verification of WAL segments

### Design Principles
1. **Single Source of Truth** - UBSCore owns all balance state
2. **Battle-Tested Code** - Use proven GlobalLedger implementation
3. **Zero Downtime** - Migration path that doesn't break existing system
4. **Performance** - Maintain current throughput

---

## 📋 INTEGRATION PLAN

### Phase 1: Compatibility Layer (Current - Quick Win)
**Status:** ✅ COMPLETE
- UBSCore uses its own HashMap
- ME uses NullLedger (trusts UBSCore)
- Settlement only tracks deltas
- **Works but no durability**

### Phase 2: Add GlobalLedger to UBSCore (Recommended Next)
**Effort:** Medium (2-3 days)
**Risk:** Low (can run in parallel with existing HashMap)

**Steps:**
1. Add GlobalLedger as field in UBSCore
   ```rust
   pub struct UBSCore<R: RiskModel> {
       ledger: GlobalLedger,  // NEW
       // accounts: HashMap<UserId, UserAccount>,  // REMOVE
       // ... rest stays same
   }
   ```

2. Update all balance operations to use ledger
   ```rust
   // BEFORE:
   self.accounts.get_mut(&user_id)

   // AFTER:
   self.ledger.get_account(&user_id)
   ```

3. Initialize GlobalLedger on startup
   ```rust
   let ledger = GlobalLedger::new(wal_dir, snap_dir)?;
   UBSCore { ledger, ... }
   ```

4. Enable snapshotting (periodic or on shutdown)
   ```rust
   // Every N operations or time interval
   if self.ledger.sequence() % SNAPSHOT_INTERVAL == 0 {
       self.ledger.snapshot()?;
   }
   ```

### Phase 3: Unified WAL (Future Enhancement)
**Effort:** High (1 week)
**Risk:** Medium

**Goal:** Merge `GroupCommitWal` (orders) and `RollingWal` (balances) into single WAL

**Benefits:**
- Single replay path for complete state recovery
- Atomic ordering of order+balance events
- Simplified architecture

---

## 🔧 IMPLEMENTATION DETAILS

### File Changes Required

#### 1. `src/ubs_core/core.rs`
```rust
// Add import
use crate::ledger::GlobalLedger;

pub struct UBSCore<R: RiskModel> {
    ledger: GlobalLedger,  // Replace accounts HashMap
    // ... rest unchanged
}

impl<R: RiskModel> UBSCore<R> {
    pub fn new(
        wal_dir: &Path,
        snap_dir: &Path,
        risk_model: R,
    ) -> Result<Self> {
        let ledger = GlobalLedger::new(wal_dir, snap_dir)?;
        Ok(UBSCore {
            ledger,
            // ... rest
        })
    }

    // Update all account access methods
    pub fn get_balance(&self, user_id: UserId, asset_id: AssetId) -> u64 {
        self.ledger.get_balance(user_id, asset_id)
    }

    pub fn deposit(&mut self, user_id: UserId, asset_id: AssetId, amount: u64) -> Result<()> {
        self.ledger.apply(&LedgerCommand::Deposit {
            user_id,
            asset_id,
            amount,
            // ...
        })
    }
}
```

#### 2. `src/bin/ubscore_aeron_service.rs`
```rust
// Update BusinessState initialization
let ubs_core = UBSCore::new(
    Path::new("~/ubscore_data/wal"),
    Path::new("~/ubscore_data/snapshots"),
    SpotRiskModel::default(),
)?;
```

### Data Migration
**Current State:** No data to migrate (balances only in memory)
**Future State:** All balances persisted in GlobalLedger WAL/snapshots

**Migration Path:**
1. On first startup with GlobalLedger, balances start from zero
2. Users deposit again (or import from backup if available)
3. Alternative: Pre-populate GlobalLedger from current Settlement DB state

---

## ✅ BENEFITS OF INTEGRATION

### 1. Crash Recovery
**Before:** Restart loses all balance state
**After:** Replay WAL + load snapshot → full state recovery in seconds

### 2. Audit Trail
**Before:** No history of balance changes
**After:** Complete WAL log of every deposit/withdraw/trade

### 3. Data Integrity
**Before:** No verification
**After:** MD5 checksums on WAL segments, detect corruption

### 4. Performance
**Before:** In-memory HashMap (fast but fragile)
**After:** GlobalLedger with write-behind WAL (fast + durable)

### 5. Testing
**Before:** Mocked balances in tests
**After:** Use real GlobalLedger in tests (same code as production)

---

## 🚧 RISKS & MITIGATIONS

### Risk 1: Performance Regression
**Concern:** WAL writes might slow down balance operations
**Mitigation:**
- Use `append_no_flush()` for batching
- Flush only on commit boundaries
- Benchmark before/after

### Risk 2: Disk Space
**Concern:** WAL segments accumulate over time
**Mitigation:**
- Automatic WAL cleanup (keep last N segments)
- Periodic snapshots (compact state)
- Already implemented in GlobalLedger!

### Risk 3: Breaking Changes
**Concern:** Existing UBSCore code expects HashMap
**Mitigation:**
- GlobalLedger implements same Ledger trait
- Minimal code changes (mostly s/accounts/ledger/)
- Gradual rollout (feature flag)

---

## 📊 COMPARISON

| Feature | Current (HashMap) | With GlobalLedger |
|---------|-------------------|-------------------|
| **Durability** | ❌ Lost on crash | ✅ WAL persisted |
| **Recovery** | ❌ Manual rebuild | ✅ Auto replay |
| **Audit** | ❌ No history | ✅ Complete log |
| **Integrity** | ❌ No verification | ✅ MD5 checksums |
| **Performance** | ✅ Fast (memory) | ✅ Fast (write-behind) |
| **Testing** | ⚠️ Mocked | ✅ Real code |
| **Code Complexity** | ✅ Simple | ⚠️ More complex |

---

## 🎯 RECOMMENDATION

### Immediate Action (Today)
1. ✅ Document current state (this file)
2. ⏭️ Create feature branch `integrate-global-ledger`
3. ⏭️ Write integration tests first
4. ⏭️ Implement Phase 2 (Add GlobalLedger to UBSCore)

### Timeline
- **Day 1:** Test setup + basic integration
- **Day 2:** Replace all HashMap accesses
- **Day 3:** E2E testing + performance validation
- **Day 4:** Code review + merge

### Success Criteria
- ✅ All existing tests pass
- ✅ New tests for crash recovery pass
- ✅ Performance within 10% of baseline
- ✅ WAL files created and verified
- ✅ Snapshot/replay working

---

## 📝 NEXT STEPS

### For Developer
1. Read `src/ledger.rs` - understand GlobalLedger interface
2. Check GlobalLedger tests - see usage examples
3. Create `integrate-global-ledger` branch
4. Write failing test for UBSCore with GlobalLedger
5. Make test pass (TDD approach)

### Questions to Resolve
- [ ] Where should wal_dir and snap_dir be configured?
- [ ] What snapshot interval? (time-based or operation-based?)
- [ ] Should we keep old GroupCommitWal or merge into GlobalLedger?
- [ ] Data migration strategy for existing deployments?

---

## 🔗 RELATED FILES

- `src/ledger.rs` - GlobalLedger implementation (1854 lines)
- `src/ubs_core/core.rs` - UBSCore main logic (needs integration)
- `src/bin/ubscore_aeron_service.rs` - UBSCore service entry point
- `tests/` - GlobalLedger tests (examples of usage)

---

**Status:** 📋 READY FOR IMPLEMENTATION
**Priority:** HIGH (durability is critical for production)
**Complexity:** MEDIUM (well-defined, low risk)

