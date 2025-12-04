#!/bin/bash
# scripts/init_starrocks.sh

echo "Initializing StarRocks schema..."

# Check if container is running
if ! docker ps | grep -q starrocks; then
    echo "StarRocks container is not running. Please run 'docker-compose up -d starrocks'."
    exit 1
fi

# Wait for StarRocks to be ready
MAX_RETRIES=30
RETRY_DELAY=2

for i in $(seq 1 $MAX_RETRIES); do
    if docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "SELECT 1" >/dev/null 2>&1; then
        echo "StarRocks is ready!"
        break
    fi
    echo "Waiting for StarRocks... ($i/$MAX_RETRIES)"
    sleep $RETRY_DELAY
done

# Copy schema to container
docker cp schema/starrocks_schema.sql starrocks:/tmp/starrocks_schema.sql

# Apply schema
docker exec starrocks mysql -h 127.0.0.1 -P 9030 -u root -e "source /tmp/starrocks_schema.sql"

if [ $? -eq 0 ]; then
    echo "Schema initialized successfully."
else
    echo "Failed to initialize schema."
    exit 1
fi
