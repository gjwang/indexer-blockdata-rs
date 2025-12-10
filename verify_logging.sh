#!/bin/bash
# Quick verification of Phase 3 async logging
# Updated to check dated log files (*.log.YYYY-MM-DD)

echo "🔍 Phase 3 Logging Verification"
echo "================================"
echo ""

# Helper: Get the most recent log file for a service
get_latest_log() {
    local service=$1
    # Check for dated logs first (*.log.*)
    local dated=$(ls -t logs/${service}.log.* 2>/dev/null | head -1)
    if [ -n "$dated" ] && [ -f "$dated" ]; then
        echo "$dated"
    else
        echo "logs/${service}.log"
    fi
}

# 1. Check log files exist
echo "1️⃣ Checking log files..."
if [ -d "logs" ]; then
    echo "✅ logs/ directory exists"
    echo ""
    echo "Base log files:"
    ls -lh logs/*.log 2>/dev/null | tail -5
    echo ""
    echo "Dated log files (active):"
    ls -lh logs/*.log.* 2>/dev/null | tail -5
    echo ""
else
    echo "❌ logs/ directory not found"
    exit 1
fi

# 2. Check JSON format (check dated files)
echo "2️⃣ Verifying JSON format..."
for service in ubscore gateway matching_engine settlement; do
    logfile=$(get_latest_log $service)
    if [ -f "$logfile" ]; then
        echo "Checking $logfile..."
        if head -1 "$logfile" 2>/dev/null | jq . > /dev/null 2>&1; then
            echo "✅ Valid JSON"
        else
            # Check if it's text logging (Settlement uses env_logger)
            if [ "$service" = "settlement" ]; then
                echo "ℹ️  Text format (env_logger - intentional)"
            else
                echo "⚠️  Not JSON or empty"
            fi
        fi
    else
        echo "⚠️  Log file not found: $logfile"
    fi
done
echo ""

# 3. Check for event IDs (check dated files)
echo "3️⃣ Checking event tracking..."
ubscore_log=$(get_latest_log ubscore)
if grep -q "event_id=" "$ubscore_log" 2>/dev/null; then
    echo "✅ Event IDs found in UBSCore"
    echo "Sample events:"
    grep "event_id=" "$ubscore_log" 2>/dev/null | head -3 | cut -c1-120
else
    echo "⚠️  No event IDs found in $ubscore_log"
fi
echo ""

# 4. Check deposit lifecycle (check all dated files)
echo "4️⃣ Checking deposit lifecycle..."
FIRST_DEPOSIT=$(grep -h "DEPOSIT_CONSUMED" logs/*.log.* logs/*.log 2>/dev/null | head -1 | grep -o "event_id=[^ ]*" | cut -d= -f2)
if [ -n "$FIRST_DEPOSIT" ]; then
    echo "Found deposit: $FIRST_DEPOSIT"
    echo "Lifecycle:"
    grep "$FIRST_DEPOSIT" logs/*.log.* logs/*.log 2>/dev/null | head -10 | cut -c1-150
else
    echo "⚠️  No deposits found yet"
fi
echo ""

# 5. Service startup messages (check dated files)
echo "5️⃣ Service startup messages..."
echo "UBSCore:"
grep "UBSCore Service starting" logs/ubscore.log.* logs/ubscore.log 2>/dev/null | tail -1 | cut -c1-120
echo "Settlement:"
grep "Settlement Service starting" logs/settlement.log 2>/dev/null | tail -1 | cut -c1-120
echo "Matching Engine:"
grep "Matching Engine starting" logs/matching_engine.log.* logs/matching_engine.log 2>/dev/null | tail -1 | cut -c1-120
echo "Gateway:"
grep "Gateway starting" logs/gateway.log.* logs/gateway.log 2>/dev/null | tail -1 | cut -c1-120
echo ""

# 6. Summary
echo "📊 Summary"
echo "=========="
echo "Log files:"
echo "  Base files:  $(ls logs/*.log 2>/dev/null | wc -l)"
echo "  Dated files: $(ls logs/*.log.* 2>/dev/null | wc -l)"
total_size=$(du -sh logs 2>/dev/null | awk '{print $1}')
echo "  Total size:  $total_size"
echo ""

# 7. Database verification
echo "7️⃣ Database verification..."
if command -v docker &> /dev/null; then
    echo "Checking trades:"
    docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.settled_trades;" 2>/dev/null | grep -A 1 "count" | tail -2 || echo "  ⚠️  Could not query database"
    echo ""
    echo "Checking balance ledger:"
    docker exec scylla cqlsh -e "SELECT COUNT(*) FROM trading.balance_ledger;" 2>/dev/null | grep -A 1 "count" | tail -2 || echo "  ⚠️  Could not query database"
else
    echo "⚠️  Docker not available, skipping DB checks"
fi
echo ""

echo "✅ Verification complete!"
echo ""
echo "💡 Try these commands:"
echo "  # Monitor real-time (today's logs):"
echo "  tail -f logs/*.log.$(date +%Y-%m-%d) | jq -C ."
echo ""
echo "  # Filter events from UBSCore:"
echo "  cat logs/ubscore.log.$(date +%Y-%m-%d) | jq 'select(.fields.message | contains(\"event_id\"))'"
echo ""
echo "  # Find all deposits across all logs:"
echo "  grep -h 'DEPOSIT' logs/*.log.* logs/*.log 2>/dev/null | head -20"
echo ""
echo "  # Check Settlement progress:"
echo "  grep 'PROGRESS' logs/settlement.log"
echo ""
