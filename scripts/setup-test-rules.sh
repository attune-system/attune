#!/bin/bash
set -e

# Script to create test rules for Attune
# 1. Echo every second
# 2. Sleep for 3 seconds every 5 seconds
# 3. HTTP POST to httpbin.org every 10 seconds

API_URL="${ATTUNE_API_URL:-http://localhost:8080}"
LOGIN="${ATTUNE_LOGIN:-test@attune.local}"
PASSWORD="${ATTUNE_PASSWORD:-TestPass123!}"

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

echo "=== Attune Test Rules Setup ==="
echo "API origin: $API_URL_ORIGIN"
echo "Login: $LOGIN"
echo ""

# Authenticate
echo "Authenticating..."
TOKEN=$(curl -s -X POST "$API_URL/auth/login" \
  -H "Content-Type: application/json" \
  -d "{\"login\":\"$LOGIN\",\"password\":\"$PASSWORD\"}" | jq -r '.data.access_token')

if [ -z "$TOKEN" ] || [ "$TOKEN" = "null" ]; then
  echo "ERROR: Failed to authenticate"
  exit 1
fi

echo "✓ Authenticated"
echo ""

# Check if core pack exists
echo "Checking core pack..."
PACK_EXISTS=$(curl -s "$API_URL/api/v1/packs" \
  -H "Authorization: Bearer $TOKEN" | jq -r '.data[] | select(.ref == "core") | .ref')

if [ "$PACK_EXISTS" != "core" ]; then
  echo "ERROR: Core pack not found"
  exit 1
fi

echo "✓ Core pack found"
echo ""

# Create Rule 1: Echo every second
echo "Creating Rule 1: Echo every 1 second..."
RULE1=$(curl -s -X POST "$API_URL/api/v1/rules" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "test.echo_every_second",
    "label": "Echo Every Second",
    "description": "Echoes a message every second using interval timer",
    "pack_ref": "core",
    "action_ref": "core.echo",
    "trigger_ref": "core.intervaltimer",
    "enabled": true,
    "trigger_params": {
      "unit": "seconds",
      "interval": 1
    },
    "action_params": {
      "message": "Hello from 1-second timer! Time: {{event.payload.executed_at}}"
    }
  }')

RULE1_ID=$(echo "$RULE1" | jq -r '.data.id // .id // empty')
if [ -z "$RULE1_ID" ]; then
  echo "ERROR: Failed to create rule 1"
  echo "Rule creation response body omitted"
  exit 1
fi

echo "✓ Rule 1 created (ID: $RULE1_ID)"
echo ""

# Create Rule 2: Sleep 3 seconds every 5 seconds
echo "Creating Rule 2: Sleep 3 seconds every 5 seconds..."
RULE2=$(curl -s -X POST "$API_URL/api/v1/rules" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "test.sleep_every_5s",
    "label": "Sleep Every 5 Seconds",
    "description": "Sleeps for 3 seconds every 5 seconds",
    "pack_ref": "core",
    "action_ref": "core.sleep",
    "trigger_ref": "core.intervaltimer",
    "enabled": true,
    "trigger_params": {
      "unit": "seconds",
      "interval": 5
    },
    "action_params": {
      "seconds": 3
    }
  }')

RULE2_ID=$(echo "$RULE2" | jq -r '.data.id // .id // empty')
if [ -z "$RULE2_ID" ]; then
  echo "ERROR: Failed to create rule 2"
  echo "Rule creation response body omitted"
  exit 1
fi

echo "✓ Rule 2 created (ID: $RULE2_ID)"
echo ""

# Create Rule 3: HTTP POST to httpbin.org every 10 seconds
echo "Creating Rule 3: HTTP POST to httpbin.org every 10 seconds..."
RULE3=$(curl -s -X POST "$API_URL/api/v1/rules" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "test.httpbin_post",
    "label": "HTTPBin POST Every 10 Seconds",
    "description": "Makes a POST request to httpbin.org every 10 seconds",
    "pack_ref": "core",
    "action_ref": "core.http_request",
    "trigger_ref": "core.intervaltimer",
    "enabled": true,
    "trigger_params": {
      "unit": "seconds",
      "interval": 10
    },
    "action_params": {
      "url": "https://httpbin.org/post",
      "method": "POST",
      "body": "{\"message\": \"Test from Attune\", \"timestamp\": \"{{event.payload.executed_at}}\", \"rule\": \"test.httpbin_post\"}",
      "headers": {
        "Content-Type": "application/json",
        "User-Agent": "Attune-Test/1.0"
      }
    }
  }')

RULE3_ID=$(echo "$RULE3" | jq -r '.data.id // .id // empty')
if [ -z "$RULE3_ID" ]; then
  echo "ERROR: Failed to create rule 3"
  echo "Rule creation response body omitted"
  exit 1
fi

echo "✓ Rule 3 created (ID: $RULE3_ID)"
echo ""

# List all rules
echo "=== Created Rules ==="
curl -s "$API_URL/api/v1/rules" \
  -H "Authorization: Bearer $TOKEN" | jq -r '.data[] | select(.ref | startswith("test.")) | "  - \(.ref) (\(.label)) - Enabled: \(.enabled)"'

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Rules have been created and enabled."
echo "Monitor executions with:"
echo "  curl -s ${API_URL_ORIGIN}/api/v1/executions -H \"Authorization: Bearer <token>\" | jq '.data[] | {id, action_ref, status, created}'"
echo ""
echo "Or view in the web UI at http://localhost:3000"
echo ""
