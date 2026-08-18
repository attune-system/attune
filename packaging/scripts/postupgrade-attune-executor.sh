#!/bin/sh
set -e

postinstall_service=${ATTUNE_PACKAGE_POSTINSTALL_SERVICE:-/usr/lib/attune/package-hooks/postinstall-service.sh}
sh "$postinstall_service" configure "${2:-upgrade}" split -- attune-executor
