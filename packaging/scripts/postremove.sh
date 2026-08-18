#!/bin/sh
# Post-remove script for Attune packages
set -e

# Reload systemd after unit file removal
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
fi

# Packs, artifacts, credentials, and the shared service account can be used by
# other split packages. Never remove shared deployment state from a component
# package hook; administrators may remove it explicitly after all components.
