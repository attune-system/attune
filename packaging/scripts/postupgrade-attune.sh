#!/bin/sh
set -e

postinstall_service=${ATTUNE_PACKAGE_POSTINSTALL_SERVICE:-/usr/lib/attune/package-hooks/postinstall-service.sh}
sh "$postinstall_service" configure "${2:-upgrade}" all-in-one -- \
    attune-api attune-executor attune-supervisor \
    attune-worker attune-sensor attune-notifier

links=${ATTUNE_PACKAGE_ALL_IN_ONE_LINKS:-/usr/lib/attune/package-hooks/all-in-one-links.sh}
sh "$links" install
