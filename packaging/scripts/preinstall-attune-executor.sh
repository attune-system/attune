#!/bin/sh
set -e

capture=${ATTUNE_PACKAGE_CAPTURE_SERVICE_STATE:-/usr/lib/attune/package-hooks/capture-service-state.sh}
package_action=${1:-}
capture_mode=auto
if [ -n "${2:-}" ]; then
    case "$package_action" in upgrade|install) : ;; *) capture_mode=refresh ;; esac
    package_action=upgrade
elif [ "$package_action" != upgrade ]; then
    capture_mode=from-all-in-one
fi
sh "$capture" "$capture_mode" "$package_action" -- attune-executor
