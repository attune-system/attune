#!/usr/bin/env bash
# build-linux-packages.sh — Build .deb, .rpm, and .pkg.tar.zst packages using nfpm
#
# Usage:
#   ./scripts/build-linux-packages.sh <bundle_dir> [arch] [version]
#
# Arguments:
#   bundle_dir  Path to extracted binary bundle (containing bin/ and agent/ dirs)
#   arch        Package architecture: amd64 or arm64 (default: amd64)
#   version     Package version (default: read from Cargo.toml)
#
# Output:
#   dist/packages/*.deb
#   dist/packages/*.rpm
#   dist/packages/*.pkg.tar.zst

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PACKAGING_DIR="$PROJECT_ROOT/packaging"
NFPM_DIR="$PACKAGING_DIR/nfpm"
OUTPUT_DIR="$PROJECT_ROOT/dist/packages"

# Arguments
BUNDLE_DIR="${1:?Usage: $0 <bundle_dir> [arch] [version]}"
ARCH="${2:-amd64}"
VERSION="${3:-}"

# Resolve bundle dir to absolute path
BUNDLE_DIR="$(cd "$BUNDLE_DIR" && pwd)"

# Get version from Cargo.toml if not provided
if [ -z "$VERSION" ]; then
    VERSION=$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
fi

echo "=== Building Linux packages ==="
echo "  Bundle:  $BUNDLE_DIR"
echo "  Arch:    $ARCH"
echo "  Version: $VERSION"
echo "  Output:  $OUTPUT_DIR"
echo ""

# Check nfpm is installed
if ! command -v nfpm >/dev/null 2>&1; then
    echo "nfpm not found. Installing..."
    if command -v go >/dev/null 2>&1; then
        go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest
    else
        # Download pre-built binary
        NFPM_VERSION="2.41.1"
        case "$(uname -m)" in
            x86_64) NFPM_ARCH="x86_64" ;;
            aarch64) NFPM_ARCH="arm64" ;;
            *) echo "Unsupported host architecture: $(uname -m)"; exit 1 ;;
        esac
        curl -sLo /tmp/nfpm.tar.gz \
            "https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/nfpm_${NFPM_VERSION}_Linux_${NFPM_ARCH}.tar.gz"
        tar -xzf /tmp/nfpm.tar.gz -C /tmp nfpm
        sudo mv /tmp/nfpm /usr/local/bin/nfpm
        rm -f /tmp/nfpm.tar.gz
    fi
fi

echo "Using nfpm: $(nfpm --version 2>&1 || echo 'unknown')"

# Verify bundle contents
for binary in bin/attune-api bin/attune-executor bin/attune-notifier bin/attune-supervisor \
              bin/attune-worker bin/attune-sensor \
              agent/attune agent/attune-mcp agent/attune-agent agent/attune-sensor-agent; do
    if [ ! -f "$BUNDLE_DIR/$binary" ]; then
        echo "ERROR: Missing binary: $BUNDLE_DIR/$binary"
        exit 1
    fi
done

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Generate shell completion scripts from the packaged binary. This keeps the
# shipped completions identical to what the binary's own `completion` command
# emits, and they ride along as static package contents (no postinst hooks).
COMPLETIONS_DIR="$(mktemp -d)"
trap 'rm -rf "$COMPLETIONS_DIR"' EXIT
"$BUNDLE_DIR/agent/attune" completion bash > "$COMPLETIONS_DIR/attune.bash"
"$BUNDLE_DIR/agent/attune" completion fish > "$COMPLETIONS_DIR/attune.fish"
"$BUNDLE_DIR/agent/attune" completion zsh > "$COMPLETIONS_DIR/_attune"
echo "Generated completion scripts:"
ls -l "$COMPLETIONS_DIR"

# Map arch names
case "$ARCH" in
    amd64|x86_64) NFPM_ARCH="amd64" ;;
    arm64|aarch64) NFPM_ARCH="arm64" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

# Build packages for each nfpm config
FORMATS="deb rpm archlinux"
CONFIGS=$(find "$NFPM_DIR" -name "*.yaml" -type f | sort)

render_config() {
    local config="$1"
    local rendered_config="$2"
    local rendered

    # nfpm 2.41.1 does not expand these placeholders from the config file.
    rendered="$(<"$config")"
    rendered="${rendered//\$\{BUNDLE_DIR\}/$BUNDLE_DIR}"
    rendered="${rendered//\$\{COMPLETIONS_DIR\}/$COMPLETIONS_DIR}"
    rendered="${rendered//\$\{ARCH\}/$NFPM_ARCH}"
    rendered="${rendered//\$\{VERSION\}/$VERSION}"
    printf '%s\n' "$rendered" > "$rendered_config"
}

for config in $CONFIGS; do
    pkg_name=$(basename "$config" .yaml)
    echo ""
    echo "--- Building $pkg_name ---"

    rendered_config="$(mktemp "$NFPM_DIR/.${pkg_name}.rendered.XXXXXX")"
    render_config "$config" "$rendered_config"
    trap 'rm -f "$rendered_config"' EXIT

    for format in $FORMATS; do
        echo "  Format: $format"

        # Set extension for output naming
        case "$format" in
            deb) ext="deb" ;;
            rpm) ext="rpm" ;;
            archlinux) ext="pkg.tar.zst" ;;
        esac

        (
            # nfpm resolves relative content paths from the current directory.
            cd "$NFPM_DIR"
            nfpm package \
                --config "$rendered_config" \
                --packager "$format" \
                --target "$OUTPUT_DIR/"
        )

        echo "    ✓ Built $pkg_name.$ext"
    done

    rm -f "$rendered_config"
    trap - EXIT
done

echo ""
echo "=== Package build complete ==="
echo "Output:"
ls -lh "$OUTPUT_DIR/"
