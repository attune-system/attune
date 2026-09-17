#!/bin/sh
# HTTP Request Action - Core Pack
# Make HTTP requests to external APIs using curl
#
# This script uses pure POSIX shell without external dependencies like jq.
# It reads parameters in DOTENV format from stdin until EOF.

set -e

# Initialize variables
url=""
method="GET"
body=""
json_body=""
timeout="30"
verify_ssl="true"
auth_type="none"
auth_username=""
auth_password=""
auth_token=""
follow_redirects="true"
max_redirects="10"

# Temporary files
headers_file=$(mktemp)
query_params_file=$(mktemp)
body_file=""
temp_headers=$(mktemp)
curl_output=$(mktemp)
write_out_file=$(mktemp)

cleanup() {
    local exit_code=$?
    rm -f "$headers_file" "$query_params_file" "$temp_headers" "$curl_output" "$write_out_file"
    [ -n "$body_file" ] && [ -f "$body_file" ] && rm -f "$body_file"
    return "$exit_code"
}
trap cleanup EXIT

# Read DOTENV-formatted parameters from stdin until EOF
while IFS= read -r line; do
    [ -z "$line" ] && continue

    key="${line%%=*}"
    value="${line#*=}"

    # Remove quotes
    case "$value" in
        \"*\") value="${value#\"}"; value="${value%\"}" ;;
        \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac

    # Process parameters
    case "$key" in
        url) url="$value" ;;
        method) method="$value" ;;
        body) body="$value" ;;
        json_body) json_body="$value" ;;
        timeout) timeout="$value" ;;
        verify_ssl) verify_ssl="$value" ;;
        auth_type) auth_type="$value" ;;
        auth_username) auth_username="$value" ;;
        auth_password) auth_password="$value" ;;
        auth_token) auth_token="$value" ;;
        follow_redirects) follow_redirects="$value" ;;
        max_redirects) max_redirects="$value" ;;
        headers.*)
            printf '%s: %s\n' "${key#headers.}" "$value" >> "$headers_file"
            ;;
        query_params.*)
            printf '%s=%s\n' "${key#query_params.}" "$value" >> "$query_params_file"
            ;;
    esac
done

# Validate required
if [ -z "$url" ]; then
    printf '{"status_code":0,"headers":{},"body":"","json":null,"elapsed_ms":0,"url":"","success":false,"error":"url parameter is required"}\n'
    exit 1
fi

# Normalize method
method=$(printf '%s' "$method" | tr '[:lower:]' '[:upper:]')

# URL encode helper
url_encode() {
    printf '%s' "$1" | sed 's/ /%20/g; s/!/%21/g; s/"/%22/g; s/#/%23/g; s/\$/%24/g; s/&/%26/g; s/'\''/%27/g'
}

# Build URL with query params
final_url="$url"
if [ -s "$query_params_file" ]; then
    query_string=""
    while IFS='=' read -r param_name param_value; do
        [ -z "$param_name" ] && continue
        encoded=$(url_encode "$param_value")
        [ -z "$query_string" ] && query_string="${param_name}=${encoded}" || query_string="${query_string}&${param_name}=${encoded}"
    done < "$query_params_file"

    if [ -n "$query_string" ]; then
        case "$final_url" in
            *\?*) final_url="${final_url}&${query_string}" ;;
            *) final_url="${final_url}?${query_string}" ;;
        esac
    fi
fi

# Prepare body
if [ -n "$json_body" ]; then
    body_file=$(mktemp)
    printf '%s' "$json_body" > "$body_file"
elif [ -n "$body" ]; then
    body_file=$(mktemp)
    printf '%s' "$body" > "$body_file"
fi

# Build curl args file (avoid shell escaping issues)
curl_args=$(mktemp)
{
    printf -- '-X\n%s\n' "$method"
    printf -- '-s\n'
    # Use @file for -w to avoid xargs escape interpretation issues
    # curl's @file mode requires literal \n (two chars) not actual newlines
    printf '\\n%%{http_code}\\n%%{url_effective}\\n' > "$write_out_file"
    printf -- '-w\n@%s\n' "$write_out_file"
    printf -- '--max-time\n%s\n' "$timeout"
    printf -- '--connect-timeout\n10\n'
    printf -- '--dump-header\n%s\n' "$temp_headers"

    [ "$verify_ssl" = "false" ] && printf -- '-k\n'

    if [ "$follow_redirects" = "true" ]; then
        printf -- '-L\n'
        printf -- '--max-redirs\n%s\n' "$max_redirects"
    fi

    if [ -s "$headers_file" ]; then
        while IFS= read -r h; do
            [ -n "$h" ] && printf -- '-H\n%s\n' "$h"
        done < "$headers_file"
    fi

    case "$auth_type" in
        basic)
            [ -n "$auth_username" ] && printf -- '-u\n%s:%s\n' "$auth_username" "$auth_password"
            ;;
        bearer)
            [ -n "$auth_token" ] && printf -- '-H\nAuthorization: Bearer %s\n' "$auth_token"
            ;;
    esac

    if [ -n "$body_file" ] && [ -f "$body_file" ]; then
        [ -n "$json_body" ] && printf -- '-H\nContent-Type: application/json\n'
        printf -- '-d\n@%s\n' "$body_file"
    fi

    printf -- '%s\n' "$final_url"
} > "$curl_args"

# Execute curl
start_time=$(date +%s%3N 2>/dev/null || echo $(($(date +%s) * 1000)))

set +e
# Treat each generated line as one argument. Plain xargs splits header values and
# JSON payload references on spaces, corrupting otherwise valid requests.
xargs -d '\n' -a "$curl_args" curl > "$curl_output" 2>&1
curl_exit_code=$?
set -e

rm -f "$curl_args"

end_time=$(date +%s%3N 2>/dev/null || echo $(($(date +%s) * 1000)))
elapsed_ms=$((end_time - start_time))

# Parse output
response=$(cat "$curl_output")
total_lines=$(printf '%s\n' "$response" | wc -l)
body_lines=$((total_lines - 2))

if [ "$body_lines" -gt 0 ]; then
    body_output=$(printf '%s\n' "$response" | head -n "$body_lines")
else
    body_output=""
fi

http_code=$(printf '%s\n' "$response" | tail -n 2 | head -n 1 | tr -d '\r\n ')
effective_url=$(printf '%s\n' "$response" | tail -n 1 | tr -d '\r\n')

case "$http_code" in
    ''|*[!0-9]*) http_code=0 ;;
esac

# Handle errors
if [ "$curl_exit_code" -ne 0 ]; then
    error_msg="curl error code $curl_exit_code"
    case $curl_exit_code in
        6) error_msg="Could not resolve host" ;;
        7) error_msg="Failed to connect to host" ;;
        28) error_msg="Request timeout" ;;
        35) error_msg="SSL/TLS connection error" ;;
        52) error_msg="Empty reply from server" ;;
        56) error_msg="Failure receiving network data" ;;
    esac
    error_msg=$(printf '%s' "$error_msg" | sed 's/\\/\\\\/g; s/"/\\"/g')
    printf '{"status_code":0,"headers":{},"body":"","json":null,"elapsed_ms":%d,"url":"%s","success":false,"error":"%s"}\n' \
        "$elapsed_ms" "$final_url" "$error_msg"
    exit 1
fi

# Parse headers
headers_json="{"
first_header=true
if [ -f "$temp_headers" ]; then
    while IFS= read -r line; do
        case "$line" in HTTP/*|'') continue ;; esac

        header_name="${line%%:*}"
        header_value="${line#*:}"
        [ "$header_name" = "$line" ] && continue

        header_value=$(printf '%s' "$header_value" | sed 's/^ *//; s/ *$//; s/\r$//; s/\\/\\\\/g; s/"/\\"/g')
        header_name=$(printf '%s' "$header_name" | sed 's/\\/\\\\/g; s/"/\\"/g')

        if [ "$first_header" = true ]; then
            headers_json="${headers_json}\"${header_name}\":\"${header_value}\""
            first_header=false
        else
            headers_json="${headers_json},\"${header_name}\":\"${header_value}\""
        fi
    done < "$temp_headers"
fi
headers_json="${headers_json}}"

# Success check
success="false"
[ "$http_code" -ge 200 ] && [ "$http_code" -lt 300 ] && success="true"

# Escape body
body_escaped=$(printf '%s' "$body_output" | sed 's/\\/\\\\/g; s/"/\\"/g; s/	/\\t/g' | awk '{printf "%s\\n", $0}' | sed 's/\\n$//')

# Detect JSON
json_parsed="null"
if [ -n "$body_output" ]; then
    first_char=$(printf '%s' "$body_output" | sed 's/^[[:space:]]*//' | head -c 1)
    last_char=$(printf '%s' "$body_output" | sed 's/[[:space:]]*$//' | tail -c 1)
    case "$first_char" in
        '{'|'[')
            case "$last_char" in
                '}'|']')
                    # Compact multi-line JSON to single line to avoid breaking
                    # the worker's last-line JSON parser. In valid JSON, literal
                    # newlines only appear as whitespace outside strings (inside
                    # strings they must be escaped as \n), so tr is safe here.
                    json_parsed=$(printf '%s' "$body_output" | tr '\n' ' ' | tr '\r' ' ')
                    ;;
            esac
            ;;
    esac
fi

# Output
if [ "$json_parsed" = "null" ]; then
    printf '{"status_code":%d,"headers":%s,"body":"%s","json":null,"elapsed_ms":%d,"url":"%s","success":%s}\n' \
        "$http_code" "$headers_json" "$body_escaped" "$elapsed_ms" "$effective_url" "$success"
else
    printf '{"status_code":%d,"headers":%s,"body":"%s","json":%s,"elapsed_ms":%d,"url":"%s","success":%s}\n' \
        "$http_code" "$headers_json" "$body_escaped" "$json_parsed" "$elapsed_ms" "$effective_url" "$success"
fi

[ "$success" = "true" ]
