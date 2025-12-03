# Determinism Audit Report

## Executive Summary
The current codebase contains several sources of non-determinism that would prevent the successful deployment of a Replicated State Machine (Active-Active) architecture. While the core matching logic (FIFO) appears deterministic, the generation of IDs and timestamps relies on local system time, which will diverge across replicas.

## Critical Issues

### 1. Trade ID Generation (FATAL)
*   **Location**: `src/fast_ulid.rs`, `src/matching_engine_base.rs`
*   **Issue**: `FastUlidHalfGen::generate()` uses `SystemTime::now()` to generate Trade IDs.
*   **Impact**: Different replicas will generate different Trade IDs for the same match. This breaks output consensus.
*   **Fix**: Replace with a deterministic generator based on `(KafkaOffset, SequenceIndex)` or `KafkaTimestamp`.

### 2. Order Timestamping (MAJOR)
*   **Location**: `src/matching_engine_base.rs` (Line 526)
*   **Issue**: New `Order` objects are assigned `SystemTime::now()`.
*   **Impact**:
    *   **Matching Logic**: Safe (FIFO based on `VecDeque` insertion order).
    *   **State Consensus**: Broken. The in-memory state (Order Book) will contain different timestamps across replicas, causing state hash mismatches.
*   **Fix**: Pass the Kafka Message Timestamp through `OrderEvent` and use it for the Order timestamp.

### 3. Random Number Generation (MINOR)
*   **Location**: `src/fast_ulid.rs`
*   **Issue**: `rng.random()` is used for the random part of ULIDs.
*   **Impact**: Non-deterministic IDs.
*   **Fix**: Use a deterministic seed or remove randomness in favor of sequence numbers.

## Safe Areas
*   **HashMap Iteration**: `GlobalLedger` and `MatchingEngine` appear to use `HashMap`s safely (lookup-based or set-update based). Batch processing preserves order via `Vec`.
*   **Order Book**: Uses `BTreeMap` (Price) and `VecDeque` (Time/Sequence), which is deterministic.
*   **Snapshots**: If `GlobalLedger` or `OrderBook` state is serialized for snapshots, keys MUST be sorted (e.g. `BTreeMap` or sorted vector) to ensure identical snapshot hashes across replicas. `FxHashMap` iteration order is not stable.

## Recommendations
1.  **Refactor `OrderEvent`**: Add `timestamp: u64` field (populated from Kafka).
2.  **Refactor `MatchingEngine`**: Accept `timestamp` in `add_order` and `process_order`.
3.  **Replace ID Generator**: Implement `DeterministicIdGen` that takes `(timestamp, sequence)` and produces unique IDs without `SystemTime` or `rand`.
