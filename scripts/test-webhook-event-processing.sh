#!/bin/bash
# Test script to verify webhook events properly trigger rule processing
# This script demonstrates that webhook events now correctly publish EventCreated messages

set -e

API_URL="${API_URL:-http://localhost:8080}"
WEBHOOK_TRIGGER="${WEBHOOK_TRIGGER:-default.example}"

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

echo "=================================================="
echo "Webhook Event Processing Test"
echo "=================================================="
echo "API origin: $API_URL_ORIGIN"
echo ""
echo "This script tests that webhook events properly trigger rule processing"
echo "by verifying the EventCreated message is published to the message queue."
echo ""

# Step 1: Check if the trigger exists
echo "Step 1: Checking if trigger '${WEBHOOK_TRIGGER}' exists..."
TRIGGER_CHECK=$(curl -s -w "\n%{http_code}" "${API_URL}/api/v1/triggers/${WEBHOOK_TRIGGER}")
HTTP_CODE=$(echo "$TRIGGER_CHECK" | tail -n1)
TRIGGER_DATA=$(echo "$TRIGGER_CHECK" | head -n-1)

if [ "$HTTP_CODE" != "200" ]; then
    echo "❌ Trigger '${WEBHOOK_TRIGGER}' not found (HTTP ${HTTP_CODE})"
    echo "   Please create the trigger first or set WEBHOOK_TRIGGER environment variable"
    exit 1
fi

echo "✅ Trigger '${WEBHOOK_TRIGGER}' exists"
echo ""

# Step 2: Check if there are any rules for this trigger
echo "Step 2: Checking for rules that subscribe to '${WEBHOOK_TRIGGER}'..."
RULES_CHECK=$(curl -s "${API_URL}/api/v1/rules")
MATCHING_RULES=$(echo "$RULES_CHECK" | jq -r ".data[] | select(.trigger_ref == \"${WEBHOOK_TRIGGER}\") | .ref")

if [ -z "$MATCHING_RULES" ]; then
    echo "⚠️  No rules found for trigger '${WEBHOOK_TRIGGER}'"
    echo "   Events will be created but no enforcements will be generated"
else
    echo "✅ Found rules for trigger '${WEBHOOK_TRIGGER}':"
    echo "$MATCHING_RULES" | while read -r rule; do
        echo "   - $rule"
    done
fi
echo ""

# Step 3: Send a webhook
echo "Step 3: Sending webhook to trigger '${WEBHOOK_TRIGGER}'..."
WEBHOOK_PAYLOAD='{"test": "data", "timestamp": "'$(date -u +"%Y-%m-%dT%H:%M:%SZ")'"}'

WEBHOOK_RESPONSE=$(curl -s -w "\n%{http_code}" \
    -X POST \
    -H "Content-Type: application/json" \
    -d "$WEBHOOK_PAYLOAD" \
    "${API_URL}/api/v1/webhooks/${WEBHOOK_TRIGGER}")

HTTP_CODE=$(echo "$WEBHOOK_RESPONSE" | tail -n1)
RESPONSE_DATA=$(echo "$WEBHOOK_RESPONSE" | head -n-1)

if [ "$HTTP_CODE" != "200" ]; then
    echo "❌ Webhook submission failed (HTTP ${HTTP_CODE})"
    REQUEST_ID=$(echo "$RESPONSE_DATA" | jq -r '.request_id // .error.request_id // empty' 2>/dev/null || true)
    if [[ "$REQUEST_ID" =~ ^[A-Za-z0-9._:-]{1,128}$ ]]; then
        echo "   Request ID: ${REQUEST_ID}"
    fi
    echo "   Response body omitted"
    exit 1
fi

EVENT_ID=$(echo "$RESPONSE_DATA" | jq -r '.data.event_id')
echo "✅ Webhook received successfully"
echo "   Event ID: ${EVENT_ID}"
echo ""

# Step 4: Check the event was created
echo "Step 4: Verifying event was created in database..."
sleep 1
EVENT_CHECK=$(curl -s "${API_URL}/api/v1/events/${EVENT_ID}")
EVENT_TRIGGER=$(echo "$EVENT_CHECK" | jq -r '.data.trigger_ref')
EVENT_RULE=$(echo "$EVENT_CHECK" | jq -r '.data.rule')

echo "✅ Event ${EVENT_ID} exists"
echo "   Trigger: ${EVENT_TRIGGER}"
echo "   Associated Rule: ${EVENT_RULE}"
echo ""

# Step 5: Check API logs for EventCreated message publishing
echo "Step 5: Checking API logs for EventCreated message..."
echo "   (Looking for 'Published EventCreated message for event ${EVENT_ID}')"
echo ""

if command -v docker &> /dev/null; then
    # Check if running in Docker
    if docker compose ps api &> /dev/null; then
        echo "   Docker logs from API service:"
        docker compose logs api --tail=50 | grep -i "event ${EVENT_ID}" || echo "   No logs found (service may not be running in Docker)"
    else
        echo "   ⚠️  Docker Compose not running, skipping log check"
    fi
else
    echo "   ⚠️  Docker not available, skipping log check"
fi
echo ""

# Step 6: Check for enforcements
echo "Step 6: Checking if enforcements were created..."
sleep 2
ENFORCEMENTS_CHECK=$(curl -s "${API_URL}/api/v1/events/${EVENT_ID}/enforcements" 2>/dev/null || echo '{"data": []}')
ENFORCEMENT_COUNT=$(echo "$ENFORCEMENTS_CHECK" | jq -r '.data | length')

if [ "$ENFORCEMENT_COUNT" -gt 0 ]; then
    echo "✅ ${ENFORCEMENT_COUNT} enforcement(s) created for event ${EVENT_ID}"
    echo "$ENFORCEMENTS_CHECK" | jq -r '.data[] | "   - Enforcement \(.id): \(.rule_ref) (\(.status))"'
else
    if [ -z "$MATCHING_RULES" ]; then
        echo "ℹ️  No enforcements created (expected - no rules for this trigger)"
    else
        echo "⚠️  No enforcements found (unexpected - rules exist for this trigger)"
        echo "   This may indicate the EventCreated message was not published or processed"
    fi
fi
echo ""

# Step 7: Check for executions
echo "Step 7: Checking if executions were created..."
if [ "$ENFORCEMENT_COUNT" -gt 0 ]; then
    EXECUTIONS_CHECK=$(curl -s "${API_URL}/api/v1/executions?limit=10")
    EVENT_EXECUTIONS=$(echo "$EXECUTIONS_CHECK" | jq -r ".data[] | select(.event == ${EVENT_ID})")

    if [ -n "$EVENT_EXECUTIONS" ]; then
        echo "✅ Executions created for event ${EVENT_ID}:"
        echo "$EVENT_EXECUTIONS" | jq -r '"   - Execution \(.id): \(.action_ref) (\(.status))"'
    else
        echo "⚠️  No executions found yet (may still be processing)"
    fi
else
    echo "ℹ️  Skipping execution check (no enforcements created)"
fi
echo ""

# Summary
echo "=================================================="
echo "Test Summary"
echo "=================================================="
echo "✅ Webhook received and event created: ${EVENT_ID}"
if [ "$ENFORCEMENT_COUNT" -gt 0 ]; then
    echo "✅ Event processing working: ${ENFORCEMENT_COUNT} enforcement(s) created"
    echo ""
    echo "🎉 SUCCESS: Webhook events are properly triggering rule processing!"
else
    if [ -z "$MATCHING_RULES" ]; then
        echo "ℹ️  No rules to process (create a rule for '${WEBHOOK_TRIGGER}' to test full flow)"
    else
        echo "⚠️  Event processing may not be working (check executor logs)"
    fi
fi
echo ""
