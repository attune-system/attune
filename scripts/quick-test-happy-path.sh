#!/bin/bash
# Quick Test: Timer Echo Happy Path
# Tests the complete event flow with unified runtime detection

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
API_URL="${ATTUNE_API_URL:-http://localhost:8080}"
LOGIN="${ATTUNE_LOGIN:-test@attune.local}"
PASSWORD="${ATTUNE_PASSWORD:-TestPass123!}"

echo -e "${BLUE}=== Quick Test: Timer Echo Happy Path ===${NC}\n"

# Step 1: Authenticate
echo -e "${YELLOW}Step 1:${NC} Authenticating..."
LOGIN_RESPONSE=$(curl -s -X POST "$API_URL/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"login\":\"$LOGIN\",\"password\":\"$PASSWORD\"}")

TOKEN=$(echo "$LOGIN_RESPONSE" | jq -r '.data.access_token // empty')

if [ -z "$TOKEN" ]; then
  echo -e "${RED}✗ Authentication failed${NC}"
  echo "Login response body omitted because it may contain credentials"
  exit 1
fi

echo -e "${GREEN}✓ Authenticated${NC}\n"

# Step 2: Check core pack
echo -e "${YELLOW}Step 2:${NC} Checking core pack..."
PACK=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/packs/core")
PACK_ID=$(echo "$PACK" | jq -r '.data.id // empty')

if [ -z "$PACK_ID" ]; then
  echo -e "${RED}✗ Core pack not loaded${NC}"
  echo "Please load core pack first with: docker exec attune-api /opt/attune/scripts/load-core-pack.sh"
  exit 1
fi

echo -e "${GREEN}✓ Core pack loaded (ID: $PACK_ID)${NC}\n"

# Step 3: Check for echo action
echo -e "${YELLOW}Step 3:${NC} Checking echo action..."
ACTIONS=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/actions")
ECHO_ACTION=$(echo "$ACTIONS" | jq -r '.data[]? | select(.ref == "core.echo") | .ref')

if [ -z "$ECHO_ACTION" ]; then
  echo -e "${RED}✗ Echo action not found${NC}"
  exit 1
fi

echo -e "${GREEN}✓ Echo action found${NC}\n"

# Step 4: Check interval timer trigger
echo -e "${YELLOW}Step 4:${NC} Checking interval timer trigger..."
TRIGGERS=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/triggers")
TIMER_TRIGGER=$(echo "$TRIGGERS" | jq -r '.data[]? | select(.ref == "core.intervaltimer") | .id')

if [ -z "$TIMER_TRIGGER" ]; then
  echo -e "${RED}✗ Interval timer trigger not found${NC}"
  exit 1
fi

echo -e "${GREEN}✓ Interval timer trigger found (ID: $TIMER_TRIGGER)${NC}\n"

# Step 5: Create rule with embedded timer config
echo -e "${YELLOW}Step 5:${NC} Creating rule with 1-second timer..."
RULE_REF="core.quicktest_echo_$(date +%s)"

RULE_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/rules" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"ref\": \"$RULE_REF\",
    \"pack_ref\": \"core\",
    \"label\": \"Quick Test Echo\",
    \"description\": \"Quick test - echo every second\",
    \"enabled\": true,
    \"trigger_ref\": \"core.intervaltimer\",
    \"trigger_parameters\": {
      \"unit\": \"seconds\",
      \"interval\": 1
    },
    \"action_ref\": \"core.echo\",
    \"action_parameters\": {
      \"message\": \"Hello, World! (quick test)\"
    }
  }")

RULE_ID=$(echo "$RULE_RESPONSE" | jq -r '.data.id // empty')

if [ -z "$RULE_ID" ]; then
  echo -e "${RED}✗ Failed to create rule${NC}"
  echo "Response: $RULE_RESPONSE"
  exit 1
fi

echo -e "${GREEN}✓ Rule created (ID: $RULE_ID, Ref: $RULE_REF)${NC}\n"

# Step 6: Wait for executions
echo -e "${YELLOW}Step 6:${NC} Waiting for executions..."
echo "Waiting 15 seconds for timer to fire and actions to execute..."

EXECUTION_COUNT=0
for i in {1..5}; do
  sleep 3

  EXECS=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/executions?limit=50")
  COUNT=$(echo "$EXECS" | jq '[.data[]? | select(.action_ref == "core.echo")] | length')

  if [ "$COUNT" -gt "$EXECUTION_COUNT" ]; then
    EXECUTION_COUNT=$COUNT
    echo -e "  ${GREEN}Found $EXECUTION_COUNT execution(s)${NC} (after $((i*3))s)"
  fi
done

echo ""

if [ "$EXECUTION_COUNT" -eq 0 ]; then
  echo -e "${RED}✗ No executions found${NC}\n"

  echo "Checking events..."
  EVENTS=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/events?limit=10")
  EVENT_COUNT=$(echo "$EVENTS" | jq '.data | length // 0')
  echo "  Events created: $EVENT_COUNT"

  echo "Checking enforcements..."
  ENFORCEMENTS=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/enforcements?limit=10")
  ENFORCEMENT_COUNT=$(echo "$ENFORCEMENTS" | jq '.data | length // 0')
  echo "  Enforcements created: $ENFORCEMENT_COUNT"

  echo -e "\n${RED}TEST FAILED - Check service logs:${NC}"
  echo "  docker logs attune-sensor --tail 50 | grep -i timer"
  echo "  docker logs attune-executor --tail 50"
  echo "  docker logs attune-worker-shell --tail 50"
  echo ""
  exit 1
fi

# Step 7: Check execution status
echo -e "${YELLOW}Step 7:${NC} Verifying execution status..."
EXECS=$(curl -s -H "Authorization: Bearer $TOKEN" "$API_URL/api/v1/executions?limit=20")
SUCCESS_COUNT=$(echo "$EXECS" | jq '[.data[]? | select(.action_ref == "core.echo" and .status == "succeeded")] | length')
FAILED_COUNT=$(echo "$EXECS" | jq '[.data[]? | select(.action_ref == "core.echo" and .status == "failed")] | length')
RUNNING_COUNT=$(echo "$EXECS" | jq '[.data[]? | select(.action_ref == "core.echo" and .status == "running")] | length')

echo -e "${GREEN}✓ Total executions: $EXECUTION_COUNT${NC}"
echo -e "${GREEN}✓ Successful: $SUCCESS_COUNT${NC}"
if [ "$FAILED_COUNT" -gt 0 ]; then
  echo -e "${RED}✗ Failed: $FAILED_COUNT${NC}"
fi
if [ "$RUNNING_COUNT" -gt 0 ]; then
  echo -e "${YELLOW}⟳ Running: $RUNNING_COUNT${NC}"
fi
echo ""

# Show sample executions
echo "Sample executions:"
echo "$EXECS" | jq '.data[0:3] | .[] | select(.action_ref == "core.echo") | {id, status, action_ref, created}' 2>/dev/null || echo "  (no execution details available)"
echo ""

# Step 8: Cleanup
echo -e "${YELLOW}Step 8:${NC} Cleanup..."
curl -s -X PUT "$API_URL/api/v1/rules/$RULE_REF" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"enabled": false}' > /dev/null

echo -e "${GREEN}✓ Rule disabled${NC}\n"

# Final summary
echo -e "${BLUE}=== Test Summary ===${NC}\n"
echo -e "✓ Core pack loaded"
echo -e "✓ Timer trigger available (interval timer)"
echo -e "✓ Echo action available"
echo -e "✓ Rule created (1-second interval)"
echo -e "✓ Executions found: $EXECUTION_COUNT"
echo -e "✓ Successful executions: $SUCCESS_COUNT"
echo ""

if [ "$SUCCESS_COUNT" -gt 0 ]; then
  echo -e "${GREEN}=== HAPPY PATH TEST PASSED ===${NC}\n"
  echo "Complete event flow working:"
  echo "  Timer Sensor → Event → Rule → Enforcement → Execution → Worker → Shell Action"
  echo ""
  echo "The unified runtime detection system is functioning correctly!"
  echo "The worker successfully detected the Shell runtime and executed the echo action."
  echo ""
  exit 0
elif [ "$EXECUTION_COUNT" -gt 0 ] && [ "$RUNNING_COUNT" -gt 0 ]; then
  echo -e "${YELLOW}=== PARTIAL SUCCESS ===${NC}\n"
  echo "Executions created and some are still running."
  echo "This is expected - actions may complete after this script finishes."
  echo ""
  echo "To check final status:"
  echo "  curl -H 'Authorization: Bearer <token>' $API_URL/api/v1/executions?limit=20 | jq '.data[] | select(.action_ref == \"core.echo\") | {id, status}'"
  echo ""
  exit 0
else
  echo -e "${RED}=== TEST FAILED ===${NC}\n"
  echo "Executions created but none succeeded."
  echo ""
  echo "To debug:"
  echo "  1. Check sensor logs: docker logs attune-sensor --tail 100"
  echo "  2. Check executor logs: docker logs attune-executor --tail 100"
  echo "  3. Check worker logs: docker logs attune-worker-shell --tail 100"
  echo ""
  echo "  4. Check execution details:"
  EXEC_ID=$(echo "$EXECS" | jq -r '.data[0].id // empty')
  if [ -n "$EXEC_ID" ]; then
    echo "     curl -H 'Authorization: Bearer <token>' $API_URL/api/v1/executions/$EXEC_ID | jq ."
  fi
  echo ""
  exit 1
fi
