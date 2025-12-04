# Trade History Verification

To verify the trade history consumer:

1. **Start Services**:
   Ensure Redpanda, `matching_engine_server`, and `order_gate_server` are running.

2. **Run Consumer**:
   ```bash
   cargo run --bin trade_history_consumer
   ```
   It will wait for trades.

3. **Place Orders**:
   Use `curl` to place matching orders via the Order Gateway.

   **Sell Order:**
   ```bash
   curl -X POST http://127.0.0.1:3001/api/orders \
     -H "Content-Type: application/json" \
     -d '{
       "symbol": "BTC_USDT",
       "side": "Sell",
       "price": "50000",
       "quantity": "1.0",
       "order_type": "Limit"
     }'
   ```

   **Buy Order:**
   ```bash
   curl -X POST http://127.0.0.1:3001/api/orders \
     -H "Content-Type: application/json" \
     -d '{
       "symbol": "BTC_USDT",
       "side": "Buy",
       "price": "50000",
       "quantity": "1.0",
       "order_type": "Limit"
     }'
   ```

4. **Verify**:
   The consumer should print the trade details.
