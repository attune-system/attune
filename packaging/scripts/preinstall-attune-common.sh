#!/bin/sh
set -e

state_dir=${ATTUNE_PACKAGE_STATE_DIR:-/run/attune/package-upgrade}
mkdir -p "$state_dir"
chmod 0700 "$state_dir"

package_installed() {
    package_name=$1
    if command -v dpkg-query >/dev/null 2>&1; then
        package_status=$(dpkg-query -W -f='${Status}' "$package_name" 2>/dev/null || true)
        [ "$package_status" != "install ok installed" ] || return 0
    fi
    if command -v rpm >/dev/null 2>&1; then
        if rpm -q "$package_name" >/dev/null 2>&1; then
            return 0
        fi
    fi
    if command -v pacman >/dev/null 2>&1; then
        if pacman -Q "$package_name" >/dev/null 2>&1; then
            return 0
        fi
    fi
    return 1
}

bridge_services=
bridge_layout=
if package_installed attune; then
    bridge_layout=all-in-one
    : >"$state_dir/layout-all-in-one"
    bridge_services="attune-api attune-executor attune-supervisor attune-worker attune-sensor attune-notifier"
else
    for component in attune-api attune-executor attune-supervisor attune-notifier; do
        if package_installed "$component"; then
            bridge_layout=split
            : >"$state_dir/layout-split"
            bridge_services="$bridge_services $component"
        fi
    done
fi

# This is intentionally self-contained: on the first transition from legacy
# packages the shared package payload has not been unpacked yet.
if command -v systemctl >/dev/null 2>&1; then
    for service in attune-api attune-executor attune-supervisor \
                   attune-worker attune-sensor attune-notifier; do
        rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
            "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
            "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
        if systemctl is-active --quiet "$service" 2>/dev/null; then
            : >"$state_dir/$service.active"
        fi
        if systemctl is-enabled --quiet "$service" 2>/dev/null; then
            : >"$state_dir/$service.enabled"
        fi
        case " $bridge_services " in
            *" $service "*)
                : >"$state_dir/$service.from-$bridge_layout"
                ;;
        esac
    done
fi
