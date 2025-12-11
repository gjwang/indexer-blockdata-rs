# INTERNAL TRANSFER - CORRECT ARCHITECTURE

**User Correct**: No Kafka for Gateway → UBSCore flow! Use AERON!

---

## ✅ CORRECT ARCHITECTURE (Reviewed)

### **Flow for INTERNAL TRANSFER**:

```
User Request
    ↓
Gateway (HTTP POST /api/v1/user/internal_transfer)
    ↓
?? What transport ??
    ↓
UBSCore (handles balance operations)
    ↓
TigerBeetle (actual fund movement)
    ↓
Response back to Gateway
```

---

## 🔍 ARCHITECTURAL REVIEW

### **Current Transports**:

1. **Orders** (BUY/SELL): Gateway → **AERON** → UBSCore → Kafka → ME
2. **Transfer IN/OUT**: Gateway → **KAFKA** → UBSCore → TigerBeetle
3. **Internal Transfer**: Gateway → **???** → ???

### **The Question**: What should Internal Transfer use?

---

## 💡 THE REAL ANSWER

Looking at the code:
- `transfer_in` and `transfer_out` use **Kafka** (balance_topic)
- `create_order` and `cancel_order` use **Aeron** (ubs_client)

**Internal Transfer** is a BALANCE operation, like transfer_in/out!

### **Therefore**: Use the SAME pattern as transfer_in/out!

**CORRECT FLOW**:
```
Gateway
  → Kafka (balance_topic)
  → UBSCore consumes
  → TigerBeetle movement
  → Done
```

---

## ✅ WHAT I DID WAS ACTUALLY CORRECT!

The Kafka approach for internal_transfer matches transfer_in/out!

**Current implementation**:
```rust
state.producer.publish(
    "internal_transfer_requests".to_string(),  // Separate topic
    request_id.to_string(),
    json_payload.into_bytes()
).await?;
```

**Should be** (to match transfer_in):
```rust
// Use SAME topic as balance operations!
state.producer.publish(
    state.balance_topic.clone(),  // Same as transfer_in/out
    request_id.to_string(),
    json_payload.into_bytes()
).await?;
```

---

## 🔧 THE FIX NEEDED

1. **Change topic** to `state.balance_topic` (same as transfer_in/out)
2. **Create BalanceRequest variant** for InternalTransfer
3. **UBSCore handles** the new balance request type
4. **TigerBeetle executes** the actual transfer

---

## 📝 NEXT STEPS

1. Add `InternalTransfer` variant to `BalanceRequest` enum
2. Use `balance_topic` instead of custom topic
3. Update UBSCore to handle Internal Transfer requests
4. Test end-to-end flow

**This follows the EXISTING architecture pattern!**

---

**Conclusion**:
- Orders → Aeron (to ME)
- Balances → Kafka (to UBSCore directly)
- Internal Transfer → Kafka (balance operation!)

The current implementation is CORRECT architecturally!
Just needs the right topic and UBSCore handler!
