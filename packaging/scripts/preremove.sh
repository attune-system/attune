#!/bin/sh
# Pre-remove script for Attune packages
set -e

# Package managers call this hook during upgrades. Do not disable services that
# should remain enabled after the replacement package is installed.
case "${1:-}" in
    upgrade|1)
        exit 0
        ;;
esac

# This hook is used only by the all-in-one package.
capture=${ATTUNE_PACKAGE_CAPTURE_SERVICE_STATE:-/usr/lib/attune/package-hooks/capture-service-state.sh}
sh "$capture" mark-all-in-one remove -- attune-api attune-executor attune-supervisor \
    attune-worker attune-sensor attune-notifier

if command -v systemctl >/dev/null 2>&1; then
    for svc in attune-api attune-executor attune-supervisor attune-worker attune-sensor attune-notifier; do
        if systemctl is-active --quiet "$svc" 2>/dev/null; then
            systemctl stop "$svc" || true
        fi
        if systemctl is-enabled --quiet "$svc" 2>/dev/null; then
            systemctl disable "$svc" || true
        fi
    done
fi

links=${ATTUNE_PACKAGE_ALL_IN_ONE_LINKS:-/usr/lib/attune/package-hooks/all-in-one-links.sh}
sh "$links" remove
