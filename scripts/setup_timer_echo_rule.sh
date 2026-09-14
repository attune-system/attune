#!/bin/bash
# Setup Timer Echo Rule
# Creates a rule that runs "echo Hello World" every 10 seconds using the timer trigger

set -e

# Configuration
API_URL="${ATTUNE_API_URL:-http://localhost:8080}"
API_USER="${ATTUNE_API_USER:-admin}"
API_PASSWORD="${ATTUNE_API_PASSWORD:-admin}"

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

echo "=== Attune Timer Echo Rule Setup ==="
echo "API origin: $API_URL_ORIGIN"
echo ""

# Step 1: Login and get JWT token
echo "Step 1: Authenticating..."
LOGIN_RESPONSE=$(curl -s -X POST "$API_URL/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"$API_USER\",\"password\":\"$API_PASSWORD\"}")

ACCESS_TOKEN=$(echo "$LOGIN_RESPONSE" | jq -r '.data.access_token')

if [ "$ACCESS_TOKEN" == "null" ] || [ -z "$ACCESS_TOKEN" ]; then
  echo "Error: Failed to authenticate"
  echo "Login response body omitted because it may contain credentials"
  exit 1
fi

echo "✓ Authentication successful"
echo ""

# Step 2: Check if core pack exists
echo "Step 2: Checking for core pack..."
PACK_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/packs/core" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

PACK_ID=$(echo "$PACK_RESPONSE" | jq -r '.data.id')

if [ "$PACK_ID" == "null" ] || [ -z "$PACK_ID" ]; then
  echo "Error: Core pack not found. Please run seed_core_pack.sql first"
  echo "Pack lookup response body omitted"
  exit 1
fi

echo "✓ Core pack found (ID: $PACK_ID)"
echo ""

# Step 3: Check if timer trigger exists
echo "Step 3: Checking for timer trigger..."
TRIGGER_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/triggers/core.timer_10s" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

TRIGGER_ID=$(echo "$TRIGGER_RESPONSE" | jq -r '.data.id')

if [ "$TRIGGER_ID" == "null" ] || [ -z "$TRIGGER_ID" ]; then
  echo "Error: Timer trigger core.timer_10s not found. Please run seed_core_pack.sql first"
  echo "Trigger lookup response body omitted"
  exit 1
fi

echo "✓ Timer trigger found (ID: $TRIGGER_ID)"
echo ""

# Step 4: Check if echo action exists
echo "Step 4: Checking for echo action..."
ACTION_RESPONSE=$(curl -s -X GET "$API_URL/api/v1/actions/core.echo" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

ACTION_ID=$(echo "$ACTION_RESPONSE" | jq -r '.data.id')

if [ "$ACTION_ID" == "null" ] || [ -z "$ACTION_ID" ]; then
  echo "Error: Echo action core.echo not found. Please run seed_core_pack.sql first"
  echo "Action lookup response body omitted"
  exit 1
fi

echo "✓ Echo action found (ID: $ACTION_ID)"
echo ""

# Step 5: Create or update the rule
echo "Step 5: Creating timer echo rule..."

RULE_REF="core.timer_echo_10s"

# Check if rule already exists
EXISTING_RULE=$(curl -s -X GET "$API_URL/api/v1/rules/$RULE_REF" \
  -H "Authorization: Bearer $ACCESS_TOKEN")

EXISTING_RULE_ID=$(echo "$EXISTING_RULE" | jq -r '.data.id // empty')

if [ -n "$EXISTING_RULE_ID" ]; then
  echo "Rule already exists (ID: $EXISTING_RULE_ID), updating..."

  UPDATE_RESPONSE=$(curl -s -X PUT "$API_URL/api/v1/rules/$RULE_REF" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -d '{
      "enabled": true,
      "label": "Timer Echo Every 10 Seconds",
      "description": "Echoes Hello World every 10 seconds using timer trigger"
    }')

  RULE_ID=$(echo "$UPDATE_RESPONSE" | jq -r '.data.id')
  echo "✓ Rule updated (ID: $RULE_ID)"
else
  echo "Creating new rule..."

  CREATE_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/rules" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{
      \"ref\": \"$RULE_REF\",
      \"pack\": $PACK_ID,
      \"pack_ref\": \"core\",
      \"label\": \"Timer Echo Every 10 Seconds\",
      \"description\": \"Echoes Hello World every 10 seconds using timer trigger\",
      \"enabled\": true,
      \"trigger\": $TRIGGER_ID,
      \"trigger_ref\": \"core.timer_10s\",
      \"action\": $ACTION_ID,
      \"action_ref\": \"core.echo\",
      \"action_params\": {
        \"message\": \"Hello World from timer trigger!\"
      }
    }")

  RULE_ID=$(echo "$CREATE_RESPONSE" | jq -r '.data.id')

  if [ "$RULE_ID" == "null" ] || [ -z "$RULE_ID" ]; then
    echo "Error: Failed to create rule"
    echo "Rule creation response body omitted"
    exit 1
  fi

  echo "✓ Rule created (ID: $RULE_ID)"
fi

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Rule Details:"
echo "  Ref: $RULE_REF"
echo "  ID: $RULE_ID"
echo "  Trigger: core.timer_10s (every 10 seconds)"
echo "  Action: core.echo"
echo "  Message: Hello World from timer trigger!"
echo ""
echo "The rule is now active. The echo action will run every 10 seconds."
echo "Check logs with:"
echo "  - Sensor service logs for timer events"
echo "  - Executor service logs for enforcement/scheduling"
echo "  - Worker service logs for action execution"
echo ""
echo "To monitor executions via API:"
echo "  curl -H 'Authorization: Bearer <token>' ${API_URL_ORIGIN}/api/v1/executions"
echo ""
echo "To disable the rule:"
echo "  curl -X PUT -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \\"
echo "    -d '{\"enabled\": false}' ${API_URL_ORIGIN}/api/v1/rules/$RULE_REF"
