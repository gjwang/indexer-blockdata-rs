# Legacy Code Cleanup Plan

## Overview

Remove ZeroMQ communication and replace with Kafka for consistency and simplicity.

---

## Current Architecture (Legacy)

```
Matching Engine → ZMQ PUSH → Settlement PULL
                ↓
           (EngineOutput)
```

## Target Architecture (Modern)

```
Matching Engine → Kafka (engine.outputs) → Settlement
                ↓
           (EngineOutput serialized with bincode)
```

---

## Cleanup Tasks

### Phase 1: Replace ZMQ with Kafka in Matching Engine ✅

**File**: `src/bin/matching_engine_server.rs`

**Changes**:
1. Remove ZMQ publisher initialization
2. Add Kafka producer for engine outputs
3. Replace `zmq_pub_clone.publish_engine_output()` with Kafka send
4. Remove ZMQ sync handshake with Settlement

**Benefits**:
- Consistent messaging (everything via Kafka)
- Better reliability (Kafka persistence)
- Easier debugging (Kafka topic inspection)
- Simpler dependencies

### Phase 2: Replace ZMQ with Kafka in Settlement ✅

**File**: `src/bin/settlement_service.rs`

**Changes**:
1. Remove ZMQ subscriber setup
2. Add Kafka consumer for `engine.outputs` topic
3. Replace `receive_batch()` ZMQ logic with Kafka polling
4. Remove ZMQ sync handshake

**Benefits**:
- No more dual messaging systems
- Standard Kafka consumer group
- Automatic offset management
- Replay capability

### Phase 3: Remove ZMQ Module ✅

**Files**:
- Delete `src/zmq_publisher.rs`
- Delete `src/bin/zmq_subscriber_demo.rs`
- Remove from `src/lib.rs`

### Phase 4: Clean Config ✅

**File**: `src/configure.rs`

**Changes**:
- Remove `ZmqConfig` struct
- Remove `zeromq` field from `AppConfig`
- Update config files (remove zeromq sections)

### Phase 5: Remove Dependency ✅

**File**: `Cargo.toml`

**Changes**:
- Remove `zmq = "0.10"` dependency

---

## Migration Steps (Safe Approach)

### Step 1: Add Kafka Topic

```bash
# Create new topic for engine outputs
kafka-topics --create \
  --topic engine.outputs \
  --bootstrap-server localhost:9093 \
  --partitions 1 \
  --replication-factor 1
```

### Step 2: Update Matching Engine

Add Kafka producer to send EngineOutput:

```rust
// In matching_engine_server.rs

// Replace ZMQ publisher with Kafka producer
let producer: FutureProducer = ClientConfig::new()
    .set("bootstrap.servers", &config.kafka.broker)
    .set("linger.ms", "0")  // Immediate send for low latency
    .create()
    .expect("Producer creation error");

// In disruptor handler, replace ZMQ send:
// Before:
// zmq_pub_clone.publish_engine_output(output)

// After:
let output_bytes = bincode::serialize(&output).unwrap();
let record = FutureRecord::to("engine.outputs")
    .payload(&output_bytes)
    .key(&format!("{}", output.sequence));

tokio::runtime::Runtime::new()
    .unwrap()
    .block_on(producer.send(record, Duration::from_secs(0)))
    .map(|_| ())
    .map_err(|(e, _)| format!("Kafka send failed: {:?}", e))
```

### Step 3: Update Settlement

Replace ZMQ consumer with Kafka:

```rust
// In settlement_service.rs

// Remove setup_zmq(), add Kafka consumer
let consumer: StreamConsumer = ClientConfig::new()
    .set("group.id", "settlement_group")
    .set("bootstrap.servers", &config.kafka.broker)
    .set("enable.auto.commit", "false")
    .create()
    .expect("Consumer creation failed");

consumer.subscribe(&["engine.outputs"]).expect("Subscribe failed");

// Replace receive_batch() with Kafka polling
async fn receive_batch_kafka(
    consumer: &StreamConsumer,
    max_size: usize
) -> Vec<EngineOutput> {
    let mut batch = Vec::new();

    while batch.len() < max_size {
        match consumer.recv().await {
            Ok(message) => {
                if let Some(payload) = message.payload() {
                    if let Ok(output) = bincode::deserialize::<EngineOutput>(payload) {
                        batch.push(output);
                    }
                }
            }
            Err(_) => break,
        }
    }

    batch
}
```

### Step 4: Test After Each Change

```bash
# After ME change:
./test_step_by_step.sh
# Verify: Orders still processed, no ZMQ errors in ME logs

# After Settlement change:
./test_step_by_step.sh
# Verify: Trades persisted, balance updates work

# After cleanup:
cargo check
./test_step_by_step.sh
# Verify: Everything still works, smaller binary
```

### Step 5: Remove ZMQ Code

Only after confirming Kafka path works:
1. Delete ZMQ modules
2. Remove ZMQ config
3. Remove dependency
4. Test again

---

## Validation Checklist

After each step, verify:

- [ ] Services compile
- [ ] Services start without errors
- [ ] Orders processed by ME
- [ ] Trades received by Settlement
- [ ] Balances updated correctly
- [ ] No ZMQ errors in logs
- [ ] Kafka topics have messages

---

## Rollback Plan

If issues arise:
1. Git revert the change
2. Rebuild
3. Test original flow
4. Debug issue before retrying

---

## Benefits Summary

### Before (ZMQ)
- Two messaging systems (Kafka + ZMQ)
- Complex setup (ZMQ ports, handshakes)
- No persistence (messages lost if Settlement down)
- Hard to debug (binary protocol)

### After (Kafka Only)
- ✅ Single messaging system
- ✅ Simple configuration
- ✅ Persistence (Kafka log)
- ✅ Easy debugging (kafka-console-consumer)
- ✅ Replay capability
- ✅ Smaller binary (no ZMQ dependency)

---

## Implementation Priority

1. **High**: Replace ZMQ in ME (send to Kafka)
2. **High**: Replace ZMQ in Settlement (read from Kafka)
3. **Medium**: Test thoroughly
4. **Low**: Remove ZMQ code (cleanup)
5. **Low**: Remove ZMQ config
6. **Low**: Remove dependency

---

**Status**: Ready to implement
**Estimated Time**: 1-2 hours
**Risk**: Low (can rollback easily)
**Testing**: Step-by-step with validation
