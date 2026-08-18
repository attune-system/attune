#!/bin/sh
set -e

postinstall_service=${ATTUNE_PACKAGE_POSTINSTALL_SERVICE:-/usr/lib/attune/package-hooks/postinstall-service.sh}
sh "$postinstall_service" "${1:-}" "${2:-}" split -- attune-api
