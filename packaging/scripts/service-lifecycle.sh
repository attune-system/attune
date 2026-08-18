#!/bin/sh
set -e

action=${1:?lifecycle action is required}
shift
state_dir=${ATTUNE_PACKAGE_STATE_DIR:-/run/attune/package-upgrade}

if ! command -v systemctl >/dev/null 2>&1; then
    exit 0
fi

case "$action" in
    block)
        mkdir -p "$state_dir"
        chmod 0700 "$state_dir"
        for service do
            : >"$state_dir/$service.blocked"
        done
        ;;
    restart)
        systemctl try-restart "$@" || true
        ;;
    recover)
        for service do
            if [ -e "$state_dir/$service.blocked" ]; then
                echo "Warning: not restarting $service because automatic configuration migration was skipped." >&2
                rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                    "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
                    "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
                continue
            fi

            recorded=0
            if [ -e "$state_dir/$service.enabled" ]; then
                recorded=1
                if ! systemctl is-enabled --quiet "$service" 2>/dev/null; then
                    systemctl enable "$service" || true
                fi
            fi
            if [ -e "$state_dir/$service.active" ]; then
                recorded=1
                if systemctl is-active --quiet "$service" 2>/dev/null; then
                    systemctl try-restart "$service" || true
                else
                    systemctl start "$service" || true
                fi
            fi
            rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                "$state_dir/$service.bridge" "$state_dir/$service.from-all-in-one" \
                "$state_dir/$service.from-split"

            # No record means this was not a captured legacy upgrade. Never
            # start or enable an inactive service in that case.
            [ "$recorded" -eq 1 ] || true
        done
        rm -f "$state_dir/layout-all-in-one" "$state_dir/layout-split"
        ;;
    *)
        echo "unknown lifecycle action: $action" >&2
        exit 2
        ;;
esac
