#!/bin/bash
# Test script to verify duplicate completion notification fix
# This script runs an execution and checks logs for duplicate completion warnings

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Testing Duplicate Completion Notification Fix ==="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

cd "$PROJECT_DIR"

response_file=$(mktemp)
response_headers=$(mktemp)
trap 'rm -f "$response_file" "$response_headers"' EXIT

# Check if services are running
if ! docker compose ps | grep -q "attune-api.*running"; then
    echo -e "${YELLOW}Services not running. Starting...${NC}"
    docker compose up -d
    echo "Waiting for services to be ready..."
    sleep 15
fi

echo "Step 1: Triggering a test execution..."
echo ""

# Use the core.echo action which should be available
if ! EXEC_HTTP_STATUS=$(curl -s -D "$response_headers" -o "$response_file" -w '%{http_code}' -X POST http://localhost:8080/api/v1/executions \
  -H "Content-Type: application/json" \
  -d '{
    "action_ref": "core.echo",
    "config": {
      "message": "Testing completion notification fix"
    }
  }' 2>/dev/null); then
    EXEC_HTTP_STATUS=000
fi
EXEC_RESPONSE=$(<"$response_file")
EXEC_REQUEST_ID=$(grep -Ei -m1 '^(x-attune-request-id|x-request-id|request-id):' "$response_headers" \
    | cut -d: -f2- | tr -d '\r' | xargs || true)
[[ "$EXEC_REQUEST_ID" =~ ^[A-Za-z0-9._:-]{1,128}$ ]] || EXEC_REQUEST_ID=""

EXEC_ID=$(echo "$EXEC_RESPONSE" | grep -o '"id":[0-9]*' | cut -d':' -f2 | head -1)

if [ -z "$EXEC_ID" ]; then
    echo -e "${RED}Failed to create execution (HTTP ${EXEC_HTTP_STATUS}).${NC}"
    [[ -z "$EXEC_REQUEST_ID" ]] || echo "Request ID: $EXEC_REQUEST_ID"
    echo "Response body omitted"
    exit 1
fi

echo "Execution created with ID: $EXEC_ID"
echo ""

echo "Step 2: Waiting for execution to complete..."
sleep 5
echo ""

echo "Step 3: Checking executor logs for warnings..."
echo ""

# Check for the warning message in executor logs from last minute
WARNING_COUNT=$(docker compose logs --since 1m attune-executor 2>/dev/null | \
    grep -c "Completion notification for action .* but active_count is 0" || echo "0")

echo "Found $WARNING_COUNT duplicate completion warnings"
echo ""

if [ "$WARNING_COUNT" -gt 0 ]; then
    echo -e "${RED}❌ FAIL: Duplicate completion notifications detected!${NC}"
    echo ""
    echo "Recent executor logs:"
    docker compose logs --tail 50 attune-executor | grep -A 2 -B 2 "active_count is 0"
    exit 1
else
    echo -e "${GREEN}✅ PASS: No duplicate completion warnings found!${NC}"
fi

echo ""
echo "Step 4: Verifying execution completed successfully..."
echo ""

EXEC_STATUS=$(curl -s http://localhost:8080/api/v1/executions/$EXEC_ID | \
    grep -o '"status":"[^"]*"' | cut -d':' -f2 | tr -d '"')

if [ "$EXEC_STATUS" = "Completed" ]; then
    echo -e "${GREEN}✅ Execution completed successfully${NC}"
elif [ "$EXEC_STATUS" = "Failed" ]; then
    echo -e "${YELLOW}⚠️  Execution failed (but no duplicate warnings)${NC}"
else
    echo -e "${YELLOW}⚠️  Execution status: $EXEC_STATUS${NC}"
fi

echo ""
echo "Step 5: Checking completion notification count in logs..."
echo ""

# Count how many times execution.completed was published for this execution
COMPLETION_COUNT=$(docker compose logs --since 1m attune-executor attune-worker 2>/dev/null | \
    grep "execution.completed" | grep -c "execution.*$EXEC_ID" || echo "0")

echo "Execution completion notifications published: $COMPLETION_COUNT"

if [ "$COMPLETION_COUNT" -eq 1 ]; then
    echo -e "${GREEN}✅ Exactly one completion notification (expected)${NC}"
elif [ "$COMPLETION_COUNT" -gt 1 ]; then
    echo -e "${YELLOW}⚠️  Multiple completion notifications detected (investigating...)${NC}"
    docker compose logs --since 1m attune-executor attune-worker 2>/dev/null | \
        grep "execution.completed" | grep "execution.*$EXEC_ID"
else
    echo -e "${YELLOW}⚠️  No completion notifications found in logs (may have scrolled)${NC}"
fi

echo ""
echo "=== Test Complete ==="
echo ""
echo "Summary:"
echo "  - Execution ID: $EXEC_ID"
echo "  - Status: $EXEC_STATUS"
echo "  - Duplicate warnings: $WARNING_COUNT"
echo "  - Completion notifications: $COMPLETION_COUNT"

if [ "$WARNING_COUNT" -eq 0 ]; then
    echo ""
    echo -e "${GREEN}✅ Fix verified: No duplicate completion notifications!${NC}"
    exit 0
else
    echo ""
    echo -e "${RED}❌ Issue persists: Duplicate notifications detected${NC}"
    exit 1
fi
