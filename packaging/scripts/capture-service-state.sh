#!/bin/sh
set -e

mode=${1:?capture mode is required}
shift
package_action=${1:-}
shift || true
[ "${1:-}" = -- ] && shift
state_dir=${ATTUNE_PACKAGE_STATE_DIR:-/run/attune/package-upgrade}

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

source_recorded() {
    source_layout=$1
    shift
    for source_service do
        [ ! -e "$state_dir/$source_service.from-$source_layout" ] || return 0
    done
    return 1
}

mkdir -p "$state_dir"
chmod 0700 "$state_dir"

capture=0
case "$mode:$package_action" in
    mark-all-in-one:*|mark-split:*)
        layout=${mode#mark-}
        : >"$state_dir/layout-$layout"
        for service do
            rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
                "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
            : >"$state_dir/$service.from-$layout"
        done
        capture=1
        ;;
    from-all-in-one:upgrade|from-all-in-one:failed-upgrade|\
    from-split:upgrade|from-split:failed-upgrade)
        capture=1
        ;;
    from-all-in-one:[2-9]|from-all-in-one:[1-9][0-9]*|\
    from-split:[2-9]|from-split:[1-9][0-9]*)
        for service do
            rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
                "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
        done
        rm -f "$state_dir/layout-all-in-one" "$state_dir/layout-split"
        capture=1
        ;;
    from-all-in-one:*)
        if [ -e "$state_dir/layout-all-in-one" ] ||
           package_installed attune || source_recorded all-in-one "$@"; then
            for service do
                : >"$state_dir/$service.bridge"
            done
            capture=1
        else
            mode=auto
        fi
        ;;
    from-split:*)
        split_installed=0
        for split_package in attune-api attune-executor attune-notifier attune-supervisor; do
            if package_installed "$split_package"; then
                split_installed=1
                break
            fi
        done
        if [ -e "$state_dir/layout-split" ] || [ "$split_installed" -eq 1 ] ||
           source_recorded split "$@"; then
            for service do
                : >"$state_dir/$service.bridge"
            done
            capture=1
        else
            mode=auto
        fi
        ;;
    bridge:*)
        capture=1
        ;;
    auto:upgrade|auto:failed-upgrade)
        capture=1
        ;;
    auto:[2-9]|auto:[1-9][0-9]*)
        for service do
            rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
                "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
        done
        capture=1
        ;;
    refresh:*)
        for service do
            rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
                "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
        done
        capture=1
        ;;
    auto:*)
        # RPM passes 1 and Debian passes install for a genuinely fresh install.
        # Clear abandoned state so neither path can resurrect an old service.
        for service do
            rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
                "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
                "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
        done
        rm -f "$state_dir/layout-all-in-one" "$state_dir/layout-split"
        exit 0
        ;;
esac

# A transition mode that found no opposite layout follows normal fresh-install
# behavior rather than preserving unrelated state.
if [ "$mode" = auto ] && [ "$capture" -eq 0 ]; then
    for service do
        rm -f "$state_dir/$service.active" "$state_dir/$service.enabled" \
            "$state_dir/$service.blocked" "$state_dir/$service.bridge" \
            "$state_dir/$service.from-all-in-one" "$state_dir/$service.from-split"
    done
    rm -f "$state_dir/layout-all-in-one" "$state_dir/layout-split"
    exit 0
fi

[ "$capture" -eq 1 ] || exit 0
if ! command -v systemctl >/dev/null 2>&1; then
    exit 0
fi

for service do
    if systemctl is-active --quiet "$service" 2>/dev/null; then
        : >"$state_dir/$service.active"
    fi
    if systemctl is-enabled --quiet "$service" 2>/dev/null; then
        : >"$state_dir/$service.enabled"
    fi
done
