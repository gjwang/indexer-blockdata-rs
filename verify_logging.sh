#!/bin/bash
# Quick verification of Phase 3 async logging

echo "🔍 Phase 3 Logging Verification"
echo "================================"
echo ""

# 1. Check log files exist
echo "1️⃣ Checking log files..."
if [ -d "logs" ]; then
    echo "✅ logs/ directory exists"
    ls -lh logs/*.log 2>/dev/null
    echo ""
else
    echo "❌ logs/ directory not found"
    exit 1
fi

# 2. Check JSON format
echo "2️⃣ Verifying JSON format..."
for log in logs/*.log; do
    if [ -f "$log" ]; then
        echo "Checking $log..."
        if head -1 "$log" 2>/dev/null | jq . >/dev/null 2>&1; then
            echo "✅ Valid JSON"
        else
            echo "⚠️  Not JSON or empty"
        fi
    fi
done
echo ""

# 3. Check for event IDs
echo "3️⃣ Checking event tracking..."
if grep -q "event_id=" logs/ubscore.log 2>/dev/null; then
    echo "✅ Event IDs found in UBSCore"
    grep "event_id=" logs/ubscore.log 2>/dev/null | head -3
else
    echo "⚠️  No event IDs found in UBSCore"
fi
echo ""

# 4. Check deposit lifecycle
echo "4️⃣ Checking deposit lifecycle..."
FIRST_DEPOSIT=$(grep -h "DEPOSIT_CONSUMED" logs/ubscore.log 2>/dev/null | head -1 | grep -o "event_id=[^ ]*" | cut -d= -f2)
if [ -n "$FIRST_DEPOSIT" ]; then
    echo "Found deposit: $FIRST_DEPOSIT"
    echo "Lifecycle:"
    grep "$FIRST_DEPOSIT" logs/*.log 2>/dev/null | head -10
else
    echo "⚠️  No deposits found yet"
fi
echo ""

# 5. Service startup messages
echo "5️⃣ Service startup messages..."
echo "UBSCore:"
grep "UBSCore Service starting" logs/ubscore.log 2>/dev/null | tail -1
echo "Settlement:"
grep "Settlement Service starting" logs/settlement.log 2>/dev/null | tail -1
echo "Matching Engine:"
grep "Matching Engine starting" logs/matching_engine.log 2>/dev/null | tail -1
echo "Gateway:"
grep "Gateway starting" logs/gateway.log 2>/dev/null | tail -1
echo ""

# 6. Summary
echo "📊 Summary"
echo "=========="
total_logs=$(wc -l logs/*.log 2>/dev/null | tail -1 | awk '{print $1}')
echo "Total log lines: $total_logs"
echo "Log directory size: $(du -sh logs 2>/dev/null | awk '{print $1}')"
echo ""

echo "✅ Verification complete!"
echo ""
echo "💡 Try these commands:"
echo "  tail -f logs/*.log | jq -C .                    # Monitor real-time"
echo "  jq 'select(.fields.event)' logs/ubscore.log    # Filter events"
echo "  grep 'deposit_' logs/*.log | head -20          # Find deposits"
