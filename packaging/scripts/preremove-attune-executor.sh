#!/bin/sh
set -e

case "${1:-}" in
    upgrade|1)
        exit 0
        ;;
esac

capture=${ATTUNE_PACKAGE_CAPTURE_SERVICE_STATE:-/usr/lib/attune/package-hooks/capture-service-state.sh}
sh "$capture" mark-split remove -- attune-executor

if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet attune-executor 2>/dev/null; then
        systemctl stop attune-executor || true
    fi
    if systemctl is-enabled --quiet attune-executor 2>/dev/null; then
        systemctl disable attune-executor || true
    fi
fi
