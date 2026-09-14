#!/bin/sh
# attune-agent-wrapper.sh — Bootstrap the Attune agent in any container
#
# This script provides a simple way to start the Attune universal worker agent
# in containers where the agent binary isn't available via a shared volume.
# It first checks for a volume-mounted binary, then falls back to downloading
# it from the Attune API.
#
# Usage:
#   As a container entrypoint:
#     entrypoint: ["/opt/attune/scripts/attune-agent-wrapper.sh"]
#
#   With Docker Compose volume mount:
#     volumes:
#       - ./scripts/attune-agent-wrapper.sh:/opt/attune/scripts/attune-agent-wrapper.sh:ro
#
# Environment Variables:
#   ATTUNE_AGENT_DIR   - Directory for the agent binary (default: /opt/attune/agent)
#   ATTUNE_AGENT_URL   - URL to download the agent binary from
#                        (default: http://attune-api:8080/api/v1/agent/binary)
#   ATTUNE_AGENT_TOKEN - Bootstrap token for authenticated downloads. REQUIRED
#                        unless the agent binary is already volume-mounted at
#                        $ATTUNE_AGENT_DIR/attune-agent. The Attune API
#                        refuses anonymous downloads (returns HTTP 503) and
#                        rejects invalid tokens (HTTP 401).
#   ATTUNE_AGENT_ARCH  - Target architecture (default: auto-detected via uname -m)
#
set -e

AGENT_DIR="${ATTUNE_AGENT_DIR:-/opt/attune/agent}"
AGENT_BIN="$AGENT_DIR/attune-agent"
AGENT_URL="${ATTUNE_AGENT_URL:-http://attune-api:8080/api/v1/agent/binary}"
# SECURITY: The default URL uses plain HTTP, which is fine for internal Docker
# networking. For cross-network or production deployments, set ATTUNE_AGENT_URL
# to an HTTPS endpoint and consider setting ATTUNE_AGENT_TOKEN to authenticate.
AGENT_TOKEN="${ATTUNE_AGENT_TOKEN:-}"

sanitize_url_origin() {
    url=$1
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

AGENT_URL_ORIGIN=$(sanitize_url_origin "$AGENT_URL")

# Auto-detect architecture if not specified
if [ -z "$ATTUNE_AGENT_ARCH" ]; then
    MACHINE=$(uname -m)
    case "$MACHINE" in
        x86_64|amd64)  ATTUNE_AGENT_ARCH="x86_64" ;;
        aarch64|arm64) ATTUNE_AGENT_ARCH="aarch64" ;;
        *)
            echo "[attune] WARNING: Unknown architecture '$MACHINE', defaulting to x86_64" >&2
            ATTUNE_AGENT_ARCH="x86_64"
            ;;
    esac
fi

# Use volume-mounted binary if available
if [ -x "$AGENT_BIN" ]; then
    echo "[attune] Agent binary found at $AGENT_BIN"
    exec "$AGENT_BIN" "$@"
fi

# Download the agent binary
echo "[attune] Agent binary not found at $AGENT_BIN, downloading..."
echo "[attune]   URL origin: $AGENT_URL_ORIGIN"
echo "[attune]   Architecture: $ATTUNE_AGENT_ARCH"

# SECURITY: The API requires a bootstrap token for the binary download
# endpoint (see agent.bootstrap_token in the API config). Without one,
# the API returns 503 and the download below will fail. Warn early so
# the operator can see the cause in the logs.
if [ -z "$AGENT_TOKEN" ]; then
    echo "[attune] WARNING: ATTUNE_AGENT_TOKEN is not set." >&2
    echo "[attune]   The Attune API refuses anonymous agent binary downloads." >&2
    echo "[attune]   Set ATTUNE_AGENT_TOKEN to the value of agent.bootstrap_token" >&2
    echo "[attune]   configured on the API, or volume-mount the binary at $AGENT_BIN." >&2
fi

DOWNLOAD_URL="${AGENT_URL}?arch=${ATTUNE_AGENT_ARCH}"
mkdir -p "$AGENT_DIR"

# Build auth header if token is provided
AUTH_HEADER=""
if [ -n "$AGENT_TOKEN" ]; then
    AUTH_HEADER="X-Agent-Token: $AGENT_TOKEN"
fi

# Download with retries (agent might start before API is ready)
MAX_RETRIES=10
RETRY_DELAY=5
ATTEMPT=0

while [ $ATTEMPT -lt $MAX_RETRIES ]; do
    ATTEMPT=$((ATTEMPT + 1))

    if command -v curl >/dev/null 2>&1; then
        if [ -n "$AUTH_HEADER" ]; then
            if curl -fsSL --retry 3 --retry-delay 2 -o "$AGENT_BIN" -H "$AUTH_HEADER" "$DOWNLOAD_URL" 2>/dev/null; then
                break
            fi
        else
            if curl -fsSL --retry 3 --retry-delay 2 -o "$AGENT_BIN" "$DOWNLOAD_URL" 2>/dev/null; then
                break
            fi
        fi
    elif command -v wget >/dev/null 2>&1; then
        if [ -n "$AUTH_HEADER" ]; then
            if wget -q -O "$AGENT_BIN" --header="$AUTH_HEADER" "$DOWNLOAD_URL" 2>/dev/null; then
                break
            fi
        else
            if wget -q -O "$AGENT_BIN" "$DOWNLOAD_URL" 2>/dev/null; then
                break
            fi
        fi
    else
        echo "[attune] ERROR: Neither curl nor wget available. Cannot download agent." >&2
        echo "[attune] Install curl or wget, or mount the agent binary via volume." >&2
        exit 1
    fi

    if [ $ATTEMPT -lt $MAX_RETRIES ]; then
        echo "[attune] Download attempt $ATTEMPT/$MAX_RETRIES failed, retrying in ${RETRY_DELAY}s..."
        sleep $RETRY_DELAY
    else
        echo "[attune] ERROR: Failed to download agent binary after $MAX_RETRIES attempts." >&2
        echo "[attune] Check that the API is running and ATTUNE_AGENT_URL is correct." >&2
        exit 1
    fi
done

chmod +x "$AGENT_BIN"

# Verify the binary works
if ! "$AGENT_BIN" --version >/dev/null 2>&1; then
    echo "[attune] WARNING: Downloaded binary may not be compatible with this system." >&2
    echo "[attune]   Architecture: $ATTUNE_AGENT_ARCH ($(uname -m))" >&2
    echo "[attune]   File type: $(file "$AGENT_BIN" 2>/dev/null || echo 'unknown')" >&2
fi

echo "[attune] Agent binary downloaded successfully ($(wc -c < "$AGENT_BIN") bytes)."
echo "[attune] Starting agent..."
exec "$AGENT_BIN" "$@"
