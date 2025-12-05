#!/bin/bash
# Initialize Order History Schema in ScyllaDB

set -e

SCYLLA_HOST="${SCYLLA_HOST:-127.0.0.1}"
SCYLLA_PORT="${SCYLLA_PORT:-9042}"
KEYSPACE="${KEYSPACE:-trading}"

echo "🔧 Initializing Order History Schema..."
echo "   Host: $SCYLLA_HOST:$SCYLLA_PORT"
echo "   Keyspace: $KEYSPACE"

# Check if cqlsh is available
if ! command -v cqlsh &> /dev/null; then
    echo "❌ cqlsh not found. Please install ScyllaDB tools."
    exit 1
fi

# Create keyspace if it doesn't exist
echo "📦 Creating keyspace '$KEYSPACE' (if not exists)..."
cqlsh $SCYLLA_HOST $SCYLLA_PORT -e "
CREATE KEYSPACE IF NOT EXISTS $KEYSPACE
WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};
"

# Apply schema
echo "📋 Applying order_history_schema.cql..."
cqlsh $SCYLLA_HOST $SCYLLA_PORT -k $KEYSPACE -f schema/order_history_schema.cql

# Verify tables
echo "✅ Verifying tables..."
cqlsh $SCYLLA_HOST $SCYLLA_PORT -k $KEYSPACE -e "
DESCRIBE TABLES;
"

echo ""
echo "🎉 Order History Schema initialized successfully!"
echo ""
echo "Tables created:"
echo "  - active_orders (user_id partition)"
echo "  - order_history (user_id partition, time-ordered)"
echo "  - order_updates_stream (event sourcing)"
echo "  - order_statistics (aggregated metrics)"
