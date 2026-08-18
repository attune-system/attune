#!/bin/sh
# Download Packs Action - Core Pack
# API Wrapper for POST /api/v1/packs/download
#
# This script uses pure POSIX shell without external dependencies like jq.
# It reads parameters in DOTENV format from stdin until EOF.

set -e

# Initialize variables
packs=""
destination_dir=""
ref_spec=""
timeout="300"
verify_ssl="true"
api_url="http://localhost:8080"
api_token=""

# Read DOTENV-formatted parameters from stdin until EOF
while IFS= read -r line; do
    [ -z "$line" ] && continue

    key="${line%%=*}"
    value="${line#*=}"

    # Remove quotes if present (both single and double)
    case "$value" in
        \"*\")
            value="${value#\"}"
            value="${value%\"}"
            ;;
        \'*\')
            value="${value#\'}"
            value="${value%\'}"
            ;;
    esac

    # Process parameters
    case "$key" in
        packs)
            packs="$value"
            ;;
        destination_dir)
            destination_dir="$value"
            ;;
        ref_spec)
            ref_spec="$value"
            ;;
        timeout)
            timeout="$value"
            ;;
        verify_ssl)
            verify_ssl="$value"
            ;;
        api_url)
            api_url="$value"
            ;;
        api_token)
            api_token="$value"
            ;;
    esac
done

# Validate required parameters
if [ -z "$destination_dir" ]; then
    printf '{"downloaded_packs":[],"failed_packs":[{"source":"input","error":"destination_dir is required"}],"total_count":0,"success_count":0,"failure_count":1}\n'
    exit 1
fi

# Normalize boolean
case "$verify_ssl" in
    true|True|TRUE|yes|Yes|YES|1) verify_ssl="true" ;;
    *) verify_ssl="false" ;;
esac

# Validate timeout is numeric
case "$timeout" in
    ''|*[!0-9]*)
        timeout="300"
        ;;
esac

# Escape values for JSON
packs_escaped=$(printf '%s' "$packs" | sed 's/\\/\\\\/g; s/"/\\"/g')
destination_dir_escaped=$(printf '%s' "$destination_dir" | sed 's/\\/\\\\/g; s/"/\\"/g')

# Build JSON request body
if [ -n "$ref_spec" ]; then
    ref_spec_escaped=$(printf '%s' "$ref_spec" | sed 's/\\/\\\\/g; s/"/\\"/g')
    request_body=$(cat <<EOF
{
  "packs": $packs_escaped,
  "destination_dir": "$destination_dir_escaped",
  "ref_spec": "$ref_spec_escaped",
  "timeout": $timeout,
  "verify_ssl": $verify_ssl
}
EOF
)
else
    request_body=$(cat <<EOF
{
  "packs": $packs_escaped,
  "destination_dir": "$destination_dir_escaped",
  "timeout": $timeout,
  "verify_ssl": $verify_ssl
}
EOF
)
fi

# Create temp files for curl
temp_response=$(mktemp)
temp_headers=$(mktemp)

cleanup() {
    rm -f "$temp_response" "$temp_headers"
}
trap cleanup EXIT

# Calculate curl timeout (request timeout + buffer)
curl_timeout=$((timeout + 30))

# Make API call
http_code=$(curl -X POST \
    -H "Content-Type: application/json" \
    -H "Accept: application/json" \
    ${api_token:+-H "Authorization: Bearer ${api_token}"} \
    -d "$request_body" \
    -s \
    -w "%{http_code}" \
    -o "$temp_response" \
    --max-time "$curl_timeout" \
    --connect-timeout 10 \
    "${api_url}/api/v1/packs/download" 2>/dev/null || echo "000")

# Check HTTP status
if [ "$http_code" -ge 200 ] && [ "$http_code" -lt 300 ]; then
    # Success - extract data field from API response
    response_body=$(cat "$temp_response")

    # Try to extract .data field using simple text processing
    # If response contains "data" field, extract it; otherwise use whole response
    case "$response_body" in
        *'"data":'*)
            # Extract content after "data": up to the closing brace
            # This is a simple extraction - assumes well-formed JSON
            data_content=$(printf '%s' "$response_body" | sed -n 's/.*"data":\s*\(.*\)}/\1/p')
            if [ -n "$data_content" ]; then
                printf '%s\n' "$data_content"
            else
                cat "$temp_response"
            fi
            ;;
        *)
            cat "$temp_response"
            ;;
    esac
    exit 0
else
    # Error response - try to extract error message
    error_msg="API request failed"
    if [ -s "$temp_response" ]; then
        # Try to extract error or message field
        response_content=$(cat "$temp_response")
        case "$response_content" in
            *'"error":'*)
                error_msg=$(printf '%s' "$response_content" | sed -n 's/.*"error":\s*"\([^"]*\)".*/\1/p')
                [ -z "$error_msg" ] && error_msg="API request failed"
                ;;
            *'"message":'*)
                error_msg=$(printf '%s' "$response_content" | sed -n 's/.*"message":\s*"\([^"]*\)".*/\1/p')
                [ -z "$error_msg" ] && error_msg="API request failed"
                ;;
        esac
    fi

    # Escape error message for JSON
    error_msg_escaped=$(printf '%s' "$error_msg" | sed 's/\\/\\\\/g; s/"/\\"/g')

    cat <<EOF
{
  "downloaded_packs": [],
  "failed_packs": [{
    "source": "api",
    "error": "API call failed (HTTP $http_code): $error_msg_escaped"
  }],
  "total_count": 0,
  "success_count": 0,
  "failure_count": 1
}
EOF
    exit 1
fi
