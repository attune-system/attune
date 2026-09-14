#!/bin/sh
# inject-env.sh - Injects runtime environment variables into the Web UI
# This script runs at container startup to make environment variables available to the browser
#
# When served via nginx reverse proxy (default Docker setup), the web client
# uses relative paths: API calls go to /api/... and WebSocket to /ws/...
# Both are proxied by nginx to the respective backend services.

set -e

# Default: empty string means "use relative paths through nginx proxy"
API_URL="${API_URL:-}"
WS_URL="${WS_URL:-}"

sanitize_url_origin() {
  url=$1
  case "$url" in
    http://*) scheme=http; authority=${url#http://} ;;
    https://*) scheme=https; authority=${url#https://} ;;
    ws://*) scheme=ws; authority=${url#ws://} ;;
    wss://*) scheme=wss; authority=${url#wss://} ;;
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
      display_host=${authority#\[}; display_host=${display_host%\]}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    \[*\]:*)
      display_host=${authority#\[}; display_port=${display_host#*\]}; display_host=${display_host%%\]*}; display_port=${display_port#:}
      case "$display_host" in ''|*[!0-9A-Fa-f:.]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *:*)
      display_host=${authority%:*}; display_port=${authority##*:}
      case "$display_host" in ''|*:*|*[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac
      case "$display_port" in ''|*[!0-9]*) printf '%s\n' '<url configured>'; return ;; esac
      ;;
    *) case "$authority" in *[!A-Za-z0-9._~-]*) printf '%s\n' '<url configured>'; return ;; esac ;;
  esac
  printf '%s://%s\n' "$scheme" "$authority"
}

API_URL_ORIGIN="(relative, via nginx proxy)"
WS_URL_ORIGIN="(relative, via nginx proxy)"
[ -z "$API_URL" ] || API_URL_ORIGIN=$(sanitize_url_origin "$API_URL")
[ -z "$WS_URL" ] || WS_URL_ORIGIN=$(sanitize_url_origin "$WS_URL")

# Create runtime configuration file
cat > /usr/share/nginx/html/config/runtime-config.js <<EOF
// Runtime configuration injected at container startup
// Empty values = use relative paths via nginx reverse proxy (recommended)
window.__ATTUNE_RUNTIME_CONFIG__ = {
  apiUrl: '${API_URL}',
  wsUrl: '${WS_URL}',
  environment: '${ENVIRONMENT:-production}'
};
EOF

echo "Runtime configuration injected:"
echo "  API_URL origin: $API_URL_ORIGIN"
echo "  WS_URL origin: $WS_URL_ORIGIN"
echo "  ENVIRONMENT: ${ENVIRONMENT:-production}"
