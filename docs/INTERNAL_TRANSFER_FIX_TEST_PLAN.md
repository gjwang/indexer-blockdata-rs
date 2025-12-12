# Internal Transfer Fix Test Plan

## Unit Tests
1.  **`UBSCore::on_withdraw`**:
    *   Verify it accepts `ref_id`.
    *   Verify it correctly decrements balance.
    *   (Note: `UBSCore` might not store `ref_id` internally, but the service must pass it through to the output event).

## Integration Tests
1.  **Gateway Publisher**:
    *   Mock Kafka producer.
    *   Call `handle_transfer` with Spot->Funding.
    *   Verify message sent to `balance.operations` with correct `request_id`.

## E2E Scenario (`tests/13_spot_funding_e2e.sh`)
1.  **Setup**: Start Scylla, Kafka, TigerBeetle, UBSCore, Gateway.
2.  **Fund Spot**: Send `BalanceRequest::TransferIn` (Deposit) for User 3001 (e.g. 1000 USDT).
    *   Verify UBSCore balance is 1000.
3.  **Initiate Transfer**: HTTP POST Spot->Funding (100 USDT).
    *   Verify API returns `pending`.
    *   Verify DB status `pending`.
4.  **Wait for Process**:
    *   UBSCore consumes request -> publishes `withdraw` event (ref_id=req_id).
    *   Settlement consumes event -> Credits TB Funding. -> Updates DB.
5.  **Verify Final State**:
    *   DB status: `success`.
    *   UBSCore Balance: 900 USDT.
    *   TB Funding Balance: Increased by 100.
