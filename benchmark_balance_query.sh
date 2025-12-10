#!/bin/bash
# Quick performance benchmark for MV-based balance queries

USER_ID=1001
ITERATIONS=100

echo "📊 Benchmarking balance queries ($ITERATIONS iterations)..."
echo "Query: GET /api/user/balance?user_id=$USER_ID"
echo ""

# Warm up
curl -s "http://localhost:3001/api/user/balance?user_id=$USER_ID" > /dev/null
sleep 1

#Benchmark
START=$(perl -MTime::HiRes -e 'printf("%.0f\n", Time::HiRes::time()*1000)')

for i in $(seq 1 $ITERATIONS); do
    curl -s "http://localhost:3001/api/user/balance?user_id=$USER_ID" > /dev/null
done

END=$(perl -MTime::HiRes -e 'printf("%.0f\n", Time::HiRes::time()*1000)')

TOTAL_MS=$(( $END - $START ))
AVG_MS=$(echo "scale=2; $TOTAL_MS / $ITERATIONS" | bc)

echo "✅ Benchmark complete!"
echo ""
echo "Total time: ${TOTAL_MS}ms"
echo "Average per query: ${AVG_MS}ms"
echo ""
echo "Expected with MV: 2-5ms per query"
echo "Previous (parallel): 5-10ms per query"
echo ""

if (( $(echo "$AVG_MS < 10" | bc -l) )); then
    echo "✅ Performance GOOD (< 10ms)"
else
    echo "⚠️ Performance slower than expected"
fi
