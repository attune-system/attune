#!/bin/bash
# Test Timer Echo Happy Path (Docker Environment)
# Verifies the complete event flow with unified runtime detection

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
API_URL="${ATTUNE_API_URL:-http://localhost:8080}"
API_USER="${ATTUNE_API_USER:-admin}"
API_PASSWORD="${ATTUNE_API_PASSWORD:-admin}"
WAIT_TIME=15  # Time to wait for executions
POLL_INTERVAL=2  # How often to check for executions

sanitize_url_origin() {
  local url=$1 scheme authority
  case "$url" in
    http://*) scheme=http; authority=${url#http://} ;;
    https://*) scheme=https; authority=${url#https://} ;;
    *) printf '%s\n' '<url configured>'; return ;;
  esac
  authority=${authority%%/*}
  authority=${authority%%\?*}
  authority=${authority%%\#*}
  case "$authority" in
    *@*@*|'') printf '%s\n' '<url configured>'; return ;;
    *@*) authority=${authority#*@} ;;
  esac
  case "$authority" in
    ''|*[[:space:]]*|*\\*) printf '%s\n' '<url configured>'; return ;;
  esac
  case "$authority" in
    \[*\])
      local display_host=${authority#\[}; display_host=${display_host%\]}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    \[*\]:*)
      local display_host=${authority#\[} display_port
      display_port=${display_host#*\]}; display_host=${display_host%%\]*}; display_port=${display_port#:}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *:*)
      local display_host=${authority%:*} display_port=${authority##*:}
      case "$display_host" in ''|*:*|*[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *) case "$authority" in *[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac ;;
  esac
  printf '%s://%s\n' "$scheme" "$authority"
}

API_URL_ORIGIN=$(sanitize_url_origin "$API_URL")

echo -e "${BLUE}=== Attune Timer Echo Happy Path Test (Docker) ===${NC}"
echo "API origin: $API_URL_ORIGIN"
echo ""

# Function to print colored status
print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_info() {
    echo -e "${YELLOW}ℹ${NC} $1"
}

# Function to check if a service is healthy
check_service() {
    local service=$1
    if docker ps --format '{{.Names}}' | grep -q "^${service}$"; then
        if docker ps --filter "name=${service}" --filter "health=healthy" --format '{{.Names}}' | grep -q "^${service}$"; then
            print_status "Service $service is healthy"
            return 0
        else
            print_error "Service $service is not healthy yet"
            return 1
        fi
    else
        print_error "Service $service is not running"
        return 1
    fi
}

# Step 0: Check Docker services
echo -e "${BLUE}Step 0: Checking Docker services...${NC}"
SERVICES=("attune-api" "attune-executor" "attune-worker" "attune-sensor" "postgres" "rabbitmq")
ALL_HEALTHY=true

for service in "${SERVICES[@]}"; do
    if ! check_service "$service" 2>/dev/null; then
        ALL_HEALTHY=false
        print_info "Service $service not ready yet"
    fi
done

if [ "$ALL_HEALTHY" = false ]; then
    print_info "Some services are not ready. Waiting 10 seconds..."
    sleep 10
fi

echo ""

# Step 1: Login and get JWT token
echo -e "${BLUE}Step 1: Authenticating...${NC}"
LOGIN_RESPONSE=$(curl -s -X POST "$API_URL/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"$API_USER\",\"password\":\"$API_PASSWORD\"}")

ACCESS_TOKEN=$(echo "$LOGIN_RESPONSE" | jq -r '.data.access_token // empty')

if [ -z "$ACCESS_TOKEN" ]; then
  print_error "Failed to authenticate"
  echo "Login response body omitted because it may contain credentials"
  exit 1
fi

print_status "Authentication successful"
echo ""

# Step 2: Verify runtime detection
echo -e "${BLUE}Step 2: Verifying runtime detection...${NC}"
RUNTIMES_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/runtimes" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

SHELL_RUNTIME=$(echo "$RUNTIMES_RESPONSE" | jq -r '.data[] | select(.name == "shell" or .name == "Shell") | .name')

if [ -n "$SHELL_RUNTIME" ]; then
  print_status "Shell runtime detected: $SHELL_RUNTIME"

  # Get runtime details
  RUNTIME_DETAILS=$(echo "$RUNTIMES_RESPONSE" | jq -r ".data[] | select(.name == \"$SHELL_RUNTIME\")")
  echo "$RUNTIME_DETAILS" | jq '{name, enabled, distributions}' || echo "$RUNTIME_DETAILS"
else
  print_error "Shell runtime not found"
  echo "Available runtimes:"
  echo "$RUNTIMES_RESPONSE" | jq '.data[] | {name, enabled}'
  exit 1
fi

echo ""

# Step 3: Check if core pack exists
echo -e "${BLUE}Step 3: Checking for core pack...${NC}"
PACK_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/packs/core" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

PACK_ID=$(echo "$PACK_RESPONSE" | jq -r '.data.id // empty')

if [ -z "$PACK_ID" ]; then
  print_error "Core pack not found"
  print_info "Attempting to load core pack..."

  # Try to load core pack via docker exec
  if docker ps --format '{{.Names}}' | grep -q "^attune-api$"; then
    docker exec attune-api /opt/attune/scripts/load-core-pack.sh || true
    sleep 2

    # Retry
    PACK_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/packs/core" \
      -H "Authorization: Bearer $ACCESS_TOKEN")
    PACK_ID=$(echo "$PACK_RESPONSE" | jq -r '.data.id // empty')

    if [ -z "$PACK_ID" ]; then
      print_error "Failed to load core pack"
      exit 1
    fi
  else
    print_error "Cannot load core pack - API container not accessible"
    exit 1
  fi
fi

print_status "Core pack found (ID: $PACK_ID)"
echo ""

# Step 4: Check interval timer trigger
echo -e "${BLUE}Step 4: Checking for interval timer trigger...${NC}"
TRIGGERS_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/triggers" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

INTERVAL_TRIGGER=$(echo "$TRIGGERS_RESPONSE" | jq -r '.data[] | select(.ref == "core.intervaltimer") | .ref')

if [ -z "$INTERVAL_TRIGGER" ]; then
  print_error "Interval timer trigger not found"
  echo "Available triggers:"
  echo "$TRIGGERS_RESPONSE" | jq '.data[] | {ref, name}'
  exit 1
fi

print_status "Interval timer trigger found"
echo ""

# Step 5: Check echo action
echo -e "${BLUE}Step 5: Checking for echo action...${NC}"
ACTIONS_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/actions" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

ECHO_ACTION=$(echo "$ACTIONS_RESPONSE" | jq -r '.data[] | select(.ref == "core.echo") | .ref')

if [ -z "$ECHO_ACTION" ]; then
  print_error "Echo action not found"
  echo "Available actions:"
  echo "$ACTIONS_RESPONSE" | jq '.data[] | {ref, name, runtime}'
  exit 1
fi

print_status "Echo action found"
ACTION_DETAILS=$(echo "$ACTIONS_RESPONSE" | jq -r '.data[] | select(.ref == "core.echo")')
echo "$ACTION_DETAILS" | jq '{ref, name, runtime, entry_point}'
echo ""

# Step 6: Create trigger instance for 1-second interval
echo -e "${BLUE}Step 6: Creating trigger instance...${NC}"

TRIGGER_INSTANCE_REF="test.timer_1s_$(date +%s)"

CREATE_TRIGGER_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/trigger-instances" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"trigger_type_ref\": \"core.intervaltimer\",
    \"ref\": \"$TRIGGER_INSTANCE_REF\",
    \"description\": \"Test timer - 1 second interval\",
    \"enabled\": true,
    \"parameters\": {
      \"unit\": \"seconds\",
      \"interval\": 1
    }
  }")

TRIGGER_INSTANCE_ID=$(echo "$CREATE_TRIGGER_RESPONSE" | jq -r '.data.id // empty')

if [ -z "$TRIGGER_INSTANCE_ID" ]; then
  print_error "Failed to create trigger instance"
  echo "Trigger creation response body omitted"
  exit 1
fi

print_status "Trigger instance created (ID: $TRIGGER_INSTANCE_ID, Ref: $TRIGGER_INSTANCE_REF)"
echo ""

# Step 7: Create rule linking timer to echo
echo -e "${BLUE}Step 7: Creating rule...${NC}"

RULE_REF="test.timer_echo_1s_$(date +%s)"

CREATE_RULE_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/rules" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"ref\": \"$RULE_REF\",
    \"pack_ref\": \"core\",
    \"name\": \"Test Timer Echo 1s\",
    \"description\": \"Test rule - echoes Hello World every second\",
    \"enabled\": true,
    \"trigger_instance_ref\": \"$TRIGGER_INSTANCE_REF\",
    \"action_ref\": \"core.echo\",
    \"action_parameters\": {
      \"message\": \"Hello, World! (from unified runtime detection test)\"
    }
  }")

RULE_ID=$(echo "$CREATE_RULE_RESPONSE" | jq -r '.data.id // empty')

if [ -z "$RULE_ID" ]; then
  print_error "Failed to create rule"
  echo "Rule creation response body omitted"
  exit 1
fi

print_status "Rule created (ID: $RULE_ID, Ref: $RULE_REF)"
echo ""

# Step 8: Wait for executions
echo -e "${BLUE}Step 8: Waiting for executions...${NC}"
print_info "Waiting $WAIT_TIME seconds for timer to fire and action to execute..."

EXECUTION_COUNT=0
START_TIME=$(date +%s)
MAX_WAIT=$((START_TIME + WAIT_TIME))

while [ $(date +%s) -lt $MAX_WAIT ]; do
  sleep $POLL_INTERVAL

  # Check for executions
  EXECUTIONS_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/executions?limit=50" \
    -H "Authorization: Bearer $ACCESS_TOKEN")

  CURRENT_COUNT=$(echo "$EXECUTIONS_RESPONSE" | jq '[.data[] | select(.action_ref == "core.echo")] | length')

  if [ "$CURRENT_COUNT" -gt "$EXECUTION_COUNT" ]; then
    EXECUTION_COUNT=$CURRENT_COUNT
    ELAPSED=$(($(date +%s) - START_TIME))
    print_status "Found $EXECUTION_COUNT execution(s) after ${ELAPSED}s"
  fi
done

echo ""

# Step 9: Verify executions
echo -e "${BLUE}Step 9: Verifying executions...${NC}"

if [ "$EXECUTION_COUNT" -eq 0 ]; then
  print_error "No executions found!"
  print_info "Checking system status..."

  # Check for events
  EVENTS_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/events?limit=10" \
    -H "Authorization: Bearer $ACCESS_TOKEN")
  EVENT_COUNT=$(echo "$EVENTS_RESPONSE" | jq '.data | length')
  echo "  Events created: $EVENT_COUNT"

  # Check for enforcements
  ENFORCEMENTS_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/enforcements?limit=10" \
    -H "Authorization: Bearer $ACCESS_TOKEN")
  ENFORCEMENT_COUNT=$(echo "$ENFORCEMENTS_RESPONSE" | jq '.data | length')
  echo "  Enforcements created: $ENFORCEMENT_COUNT"

  print_error "Happy path test FAILED - no executions"
  exit 1
fi

print_status "Found $EXECUTION_COUNT execution(s)"

# Get execution details
EXECUTIONS_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/executions?limit=5" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

echo ""
echo "Recent executions:"
echo "$EXECUTIONS_RESPONSE" | jq '.data[] | select(.action_ref == "core.echo") | {id, status, action_ref}' | head -20

# Check for successful executions
SUCCESS_COUNT=$(echo "$EXECUTIONS_RESPONSE" | jq '[.data[] | select(.action_ref == "core.echo" and .status == "succeeded")] | length')

if [ "$SUCCESS_COUNT" -gt 0 ]; then
  print_status "$SUCCESS_COUNT execution(s) succeeded"
else
  print_error "No successful executions found"
  echo ""
  echo "Execution statuses:"
  echo "$EXECUTIONS_RESPONSE" | jq '.data[] | {id, status, action_ref}'
fi

echo ""

# Step 10: Check worker logs for runtime detection
echo -e "${BLUE}Step 10: Checking worker logs for runtime execution...${NC}"
if docker ps --format '{{.Names}}' | grep -q "^attune-worker$"; then
  print_info "Recent worker logs:"
  docker logs attune-worker --tail 30 | grep -i "runtime\|shell\|echo\|executing" || echo "  (no matching log entries)"
else
  print_info "Worker container not accessible for log inspection"
fi

echo ""

# Step 11: Cleanup
echo -e "${BLUE}Step 11: Cleanup...${NC}"

# Disable the rule
print_info "Disabling rule..."
curl -s -X PUT "$API_URL/api/v1/rules/$RULE_REF" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"enabled": false}' > /dev/null

print_status "Rule disabled"

# Optionally delete the rule and trigger instance
read -p "Delete test rule and trigger instance? (y/N) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
  curl -s -X DELETE "$API_URL/api/v1/rules/$RULE_REF" \
    -H "Authorization: Bearer $ACCESS_TOKEN" > /dev/null

  curl -s -X DELETE "$API_URL/api/v1/trigger-instances/$TRIGGER_INSTANCE_REF" \
    -H "Authorization: Bearer $ACCESS_TOKEN" > /dev/null

  print_status "Test resources deleted"
else
  print_info "Test resources left in place (disabled)"
fi

echo ""

# Final summary
echo -e "${BLUE}=== Test Summary ===${NC}"
echo ""
echo "✓ Runtime detection working (Shell runtime detected)"
echo "✓ Core pack loaded with echo action"
echo "✓ Trigger instance created (1-second interval timer)"
echo "✓ Rule created and enabled"
echo "✓ Executions observed: $EXECUTION_COUNT"
echo "✓ Successful executions: $SUCCESS_COUNT"
echo ""

if [ "$SUCCESS_COUNT" -gt 0 ]; then
  echo -e "${GREEN}=== HAPPY PATH TEST PASSED ===${NC}"
  echo ""
  echo "The complete event flow is working:"
  echo "  Timer Sensor → Event → Rule → Enforcement → Execution → Worker → Shell Action"
  echo ""
  exit 0
else
  echo -e "${RED}=== HAPPY PATH TEST FAILED ===${NC}"
  echo ""
  echo "Executions were created but none succeeded."
  echo "Check service logs for errors:"
  echo "  docker logs attune-sensor"
  echo "  docker logs attune-executor"
  echo "  docker logs attune-worker"
  echo ""
  exit 1
fi
