# Internal Transfer Fix Design: Spot -> Funding

## Problem
Spot -> Funding transfers require validating and locking funds in `UBSCore` (Matching Engine) before settlement. The current implementation unsafely uses TigerBeetle. `UBSCore` has a `TransferOut` mechanism, but it drops the `request_id`, preventing the Settlement service from correlating the result.

## Solution Architecture

### 1. Data Flow
```mermaid
Gateway --(Kafka: balance.operations)--> UBSCore --(Kafka: balance.events)--> Settlement
```

### 2. Component Changes

#### A. UBSCore (`src/ubs_core/core.rs`, `src/bin/ubscore_aeron_service.rs`)
*   **Modify `on_withdraw` / `on_deposit`**: Add `ref_id: u64` parameter.
*   **Update Kafka Consumer**: Pass `request_id` from `BalanceRequest` to `on_withdraw`.
*   **Update Kafka Producer**: Include `request_id` as `ref_id` in `BalanceEvent`.

#### B. Gateway (`src/api/internal_transfer_handler.rs`)
*   **Switch Logic**:
    *   If `From == Funding`: Use TigerBeetle (Existing).
    *   If `From == Spot`: Produce `BalanceRequest::TransferOut` to `balance.operations` Kafka topic.
    *   Set status to `Pending` (or `Processing`).

#### C. Settlement (`src/api/internal_transfer_settlement.rs`)
*   **Consume `balance.events`**:
    *   Listen for `withdraw` events.
    *   Match `ref_id` to `request_id` in DB.
    *   If success: Proceed to credit Funding Account in TigerBeetle.
        *   *Note*: Since we already withdrew from Spot (UBSCore), we just need to credit Funding.
        *   TB Operation: `create_transfer` (One-way) from "Spot Mirror" to Funding?
        *   Or better: Use TB's `create_transfer` from a "System Settlement Account" to Funding.
        *   *Hypothesis*: UBSCore manages the Spot liability. When withdrawn, it's gone from Spot. We need to deposit into Funding in TB.
        *   To keep TB balanced, we might need to move from "Spot TB Account" to "Funding TB Account".

## Step-by-Step Implementation Plan

1.  **Refactor UBSCore Core**: Update methods to accept `ref_id`.
2.  **Update UBSCore Service**: Pass `request_id` through the pipeline.
3.  **Update Gateway**: Implement Kafka producer for `balance.operations`.
4.  **Update Settlement**: Implement handler for `balance.events`.
5.  **E2E Test**: Verify the full loop.

## Test Plan
*   **Unit Tests**: Verify `UBSCore` preserves `ref_id`.
*   **Integration**: Verify `Gateway` publishes to Kafka.
*   **E2E**:
    1.  Fund Spot account (via Deposit).
    2.  Request Spot -> Funding transfer.
    3.  Verify DB status transitions: `Requesting` -> `Pending` -> `Success`.
    4.  Verify Balances: Spot decreased, Funding increased.
