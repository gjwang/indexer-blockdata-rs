#!/bin/bash
# scripts/create_starrocks_routine_load.sh

echo "Creating StarRocks Routine Load job..."

# Check if container is running
if ! docker ps | grep -q starrocks; then
    echo "StarRocks container is not running. Please run 'docker-compose up -d starrocks'."
    exit 1
fi

# Define the job
# Note: We use `kafka_timestamp` to populate `settled_at` and `trade_date`.
# `settled_at` = from_unixtime(kafka_timestamp/1000)
# `trade_date` = to_date(from_unixtime(kafka_timestamp/1000))

# Kafka broker: redpanda:29092 (internal Docker network)
# Topic: trades

SQL="
CREATE ROUTINE LOAD settlement.trades_load ON trades
COLUMNS(output_sequence, trade_id, match_seq, buy_order_id, sell_order_id, buyer_user_id, seller_user_id, price, quantity, base_asset, quote_asset, buyer_refund, seller_refund),
COLUMNS(
    settled_at = from_unixtime(kafka_timestamp/1000),
    trade_date = to_date(from_unixtime(kafka_timestamp/1000))
)
PROPERTIES
(
    \"desired_concurrent_number\" = \"1\",
    \"format\" = \"json\",
    \"jsonpaths\" = \"[\\\"$.output_sequence\\\", \\\"$.trade_id\\\", \\\"$.match_seq\\\", \\\"$.buy_order_id\\\", \\\"$.sell_order_id\\\", \\\"$.buyer_user_id\\\", \\\"$.seller_user_id\\\", \\\"$.price\\\", \\\"$.quantity\\\", \\\"$.base_asset\\\", \\\"$.quote_asset\\\", \\\"$.buyer_refund\\\", \\\"$.seller_refund\\\"]\"
)
FROM KAFKA
(
    \"kafka_broker_list\" = \"redpanda:29092\",
    \"kafka_topic\" = \"trades\",
    \"property.group.id\" = \"starrocks_trades_group\",
    \"property.kafka_default_offsets\" = \"OFFSET_BEGINNING\"
);
"

echo "Submitting job..."
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "$SQL"

if [ $? -eq 0 ]; then
    echo "Routine Load job submitted successfully."
    echo "Check status with: docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e 'SHOW ROUTINE LOAD\\G'"
else
    echo "Failed to submit Routine Load job."
    exit 1
fi
