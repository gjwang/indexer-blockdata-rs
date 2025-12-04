#!/bin/bash
# Initialize ScyllaDB schema for settlement service
# This script applies the settlement schema to a running ScyllaDB instance

set -e

SCYLLA_HOST="${SCYLLA_HOST:-localhost}"
SCYLLA_PORT="${SCYLLA_PORT:-9042}"
SCHEMA_FILE="schema/settlement_schema.cql"

echo "=== ScyllaDB Settlement Schema Initialization ==="
echo "Host: $SCYLLA_HOST"
echo "Port: $SCYLLA_PORT"
echo "Schema File: $SCHEMA_FILE"
echo ""

# Check if ScyllaDB is running
echo "Checking ScyllaDB connectivity..."
if ! docker exec scylla cqlsh -e "DESCRIBE KEYSPACES" > /dev/null 2>&1; then
    echo "❌ Error: Cannot connect to ScyllaDB"
    echo "   Make sure ScyllaDB is running: docker-compose up -d scylla"
    echo "   Wait for it to be ready (may take 30-60 seconds on first start)"
    exit 1
fi

echo "✅ ScyllaDB is running"
echo ""

# Apply schema
echo "Applying settlement schema..."
if docker exec -i scylla cqlsh < "$SCHEMA_FILE"; then
    echo "✅ Schema applied successfully"
else
    echo "❌ Failed to apply schema"
    exit 1
fi

echo ""
echo "=== Verification ==="

# Verify keyspace creation
echo "Checking keyspace..."
if docker exec scylla cqlsh -e "DESCRIBE KEYSPACE settlement" > /dev/null 2>&1; then
    echo "✅ Keyspace 'settlement' created"
else
    echo "❌ Keyspace 'settlement' not found"
    exit 1
fi

# Verify table creation
echo "Checking tables..."
TABLES=$(docker exec scylla cqlsh -e "USE settlement; DESCRIBE TABLES;")
if echo "$TABLES" | grep -q "settled_trades"; then
    echo "✅ Table 'settled_trades' created"
else
    echo "❌ Table 'settled_trades' not found"
    exit 1
fi

if echo "$TABLES" | grep -q "settlement_state"; then
    echo "✅ Table 'settlement_state' created"
else
    echo "❌ Table 'settlement_state' not found"
    exit 1
fi

# Show schema summary
echo ""
echo "=== Schema Summary ==="
docker exec scylla cqlsh -e "USE settlement; DESCRIBE TABLES;"
echo ""
docker exec scylla cqlsh -e "USE settlement; DESCRIBE MATERIALIZED VIEWS;"

echo ""
echo "=== Initialization Complete ==="
echo "Settlement database is ready to use!"
