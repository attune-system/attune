#!/bin/sh
set -e

common_postinstall=${ATTUNE_PACKAGE_POSTINSTALL_COMMON:-/usr/lib/attune/package-hooks/postinstall-common.sh}
lifecycle=${ATTUNE_PACKAGE_SERVICE_LIFECYCLE:-/usr/lib/attune/package-hooks/service-lifecycle.sh}
status_file=$(mktemp "${TMPDIR:-/tmp}/attune-package-migration.XXXXXX")
trap 'rm -f "$status_file"' 0 HUP INT TERM

package_action=${1:-}
old_version=${2:-}
target_layout=${3:-}
shift 3 2>/dev/null || set --
[ "${1:-}" = -- ] && shift
state_dir=${ATTUNE_PACKAGE_STATE_DIR:-/run/attune/package-upgrade}

recover_now=0
if [ "$package_action" = configure ] && [ -n "$old_version" ]; then
    recover_now=1
else
    for service do
        if [ -e "$state_dir/$service.bridge" ] ||
           { [ "$target_layout" = split ] && [ -e "$state_dir/$service.from-all-in-one" ]; } ||
           { [ "$target_layout" = all-in-one ] && [ -e "$state_dir/$service.from-split" ]; }; then
            recover_now=1
            break
        fi
    done
fi

# RPM passes an installed-instance count to %post. Its old %preun has not run
# yet, so every lifecycle action waits for the package's %posttrans hook.
defer_to_posttrans=0
case "$package_action" in
    [1-9]|[1-9][0-9]*) defer_to_posttrans=1 ;;
esac

ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
ATTUNE_PACKAGE_MIGRATION_STATUS_FILE="$status_file" \
    sh "$common_postinstall"

if [ -s "$status_file" ]; then
    sh "$lifecycle" block "$@"
    if [ "$defer_to_posttrans" -eq 0 ] && [ "$recover_now" -eq 1 ]; then
        sh "$lifecycle" recover "$@"
    fi
    exit 0
fi

# RPM recovery waits until posttrans, after the old package's preun.
if [ "$defer_to_posttrans" -eq 1 ]; then
    exit 0
fi

if [ "$recover_now" -eq 1 ]; then
    sh "$lifecycle" recover "$@"
fi
