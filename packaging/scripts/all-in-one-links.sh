#!/bin/sh
set -e

action=${1:?link action is required}
agent_dir=${ATTUNE_PACKAGE_AGENT_DIR:-/var/lib/attune/agent}
state_dir=${ATTUNE_PACKAGE_DATA_DIR:-/var/lib/attune}
opt_dir=${ATTUNE_PACKAGE_OPT_DIR:-/opt/attune-system}

manage_link() {
    link_action=$1
    link_target=$2
    link_path=$3

    case "$link_action" in
        install)
            if [ -L "$link_path" ] && [ "$(readlink -- "$link_path")" = "$link_target" ]; then
                return 0
            fi
            if [ -e "$link_path" ] || [ -L "$link_path" ]; then
                echo "Warning: not replacing administrator-managed agent path: $link_path" >&2
                return 0
            fi
            ln -s -- "$link_target" "$link_path"
            ;;
        remove)
            if [ -L "$link_path" ] && [ "$(readlink -- "$link_path")" = "$link_target" ]; then
                rm -f -- "$link_path"
            fi
            ;;
        *)
            echo "unknown link action: $link_action" >&2
            exit 2
            ;;
    esac
}

if [ "$action" = install ]; then
    mkdir -p "$state_dir" "$agent_dir" "$opt_dir"
    chown "${ATTUNE_PACKAGE_DATA_OWNER:-attune:attune}" "$state_dir" "$agent_dir"
    chmod 0750 "$state_dir" "$agent_dir"
    chown "${ATTUNE_PACKAGE_OPT_OWNER:-root:attune}" "$opt_dir"
    chmod 0755 "$opt_dir"
fi

manage_link "$action" "$opt_dir/attune" "$agent_dir/attune"
manage_link "$action" "$opt_dir/attune-mcp" "$agent_dir/attune-mcp"
manage_link "$action" "$opt_dir/attune-agent" "$agent_dir/attune-agent"
manage_link "$action" "$opt_dir/attune-sensor-agent" "$agent_dir/attune-sensor-agent"
