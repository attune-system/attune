#!/bin/sh
# Post-install script for Attune packages
set -e

config_file=${ATTUNE_PACKAGE_CONFIG_FILE:-/etc/attune/attune.yaml}
environment_file=${ATTUNE_PACKAGE_ENVIRONMENT_FILE:-/etc/attune/environment}

migration_work_dir=
migration_file=
migration_inode=
migration_snapshot=
migration_replacement=
migration_skipped=0

mark_migration_skipped() {
    migration_skipped=1
    if [ -n "${ATTUNE_PACKAGE_MIGRATION_STATUS_FILE:-}" ]; then
        printf 'skipped\n' >"$ATTUNE_PACKAGE_MIGRATION_STATUS_FILE"
    fi
}

clear_migration_traps() {
    trap - 0 HUP INT TERM
}

clean_migration_files() {
    [ -n "$migration_work_dir" ] && rm -rf -- "$migration_work_dir" || true
    migration_work_dir=
    migration_file=
    migration_inode=
    migration_snapshot=
    migration_replacement=
}

abort_migration() {
    status=$1
    clear_migration_traps
    clean_migration_files
    exit "$status"
}

prepare_config_migration() {
    migration_file=$1
    migration_parent=$(dirname -- "$migration_file")
    migration_name=$(basename -- "$migration_file")
    migration_work_dir=$(mktemp -d "$migration_parent/.${migration_name}.migrate.XXXXXX") || return 1
    migration_inode=$migration_work_dir/inode
    migration_snapshot=$migration_work_dir/original
    migration_replacement=$migration_work_dir/replacement

    trap 'abort_migration $?' 0
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    # Pin the leaf inode without following a symbolic link. A separate physical
    # copy below is the immutable content snapshot used for transformation.
    if ! ln -P -- "$migration_file" "$migration_inode" 2>/dev/null; then
        if [ -L "$migration_file" ]; then
            echo "Warning: skipping automatic package config migration for administrator-managed symbolic link: $migration_file. Attune services will not be restarted; migrate the link target manually before restarting them." >&2
            mark_migration_skipped
            clean_migration_files
            clear_migration_traps
            return 2
        fi
        if [ ! -e "$migration_file" ]; then
            clean_migration_files
            clear_migration_traps
            return 2
        fi
        echo "Unable to snapshot package config for migration: $migration_file" >&2
        clean_migration_files
        clear_migration_traps
        return 1
    fi

    if [ -L "$migration_inode" ]; then
        echo "Warning: skipping automatic package config migration for administrator-managed symbolic link: $migration_file. Attune services will not be restarted; migrate the link target manually before restarting them." >&2
        mark_migration_skipped
        clean_migration_files
        clear_migration_traps
        return 2
    fi
    if [ ! -f "$migration_inode" ]; then
        echo "Warning: skipping automatic package config migration for non-regular path: $migration_file" >&2
        mark_migration_skipped
        clean_migration_files
        clear_migration_traps
        return 2
    fi

    # GNU cp preserves ownership, mode, ACLs, xattrs, and SELinux context when
    # the filesystem supports them. The first copy freezes content for race
    # detection; the second is the replacement modified by the migration.
    if ! /bin/cp --preserve=all -- "$migration_inode" "$migration_snapshot" ||
       ! /bin/cp --preserve=all -- "$migration_snapshot" "$migration_replacement"; then
        clean_migration_files
        clear_migration_traps
        return 1
    fi
}

publish_config_migration() {
    if cmp -s "$migration_snapshot" "$migration_replacement"; then
        clean_migration_files
        clear_migration_traps
        return 0
    fi

    touch --reference="$migration_snapshot" -- "$migration_replacement"
    sync -f "$migration_replacement"

    pinned_identity=$(stat -c '%d:%i' -- "$migration_inode")
    current_identity=$(stat -c '%d:%i' -- "$migration_file" 2>/dev/null || true)
    if [ "$current_identity" != "$pinned_identity" ] ||
       ! cmp -s "$migration_inode" "$migration_snapshot"; then
        echo "Warning: package config changed during migration; leaving it untouched: $migration_file" >&2
        mark_migration_skipped
        clean_migration_files
        clear_migration_traps
        return 0
    fi

    replacement_identity=$(stat -c '%d:%i' -- "$migration_replacement")

    rename_exchange=${ATTUNE_PACKAGE_RENAME_EXCHANGE:-/usr/lib/attune/package-hooks/rename-exchange.py}
    if ! python3 "$rename_exchange" "$migration_replacement" "$migration_file"; then
        echo "Warning: atomic package config publication is unavailable; leaving it untouched: $migration_file" >&2
        mark_migration_skipped
        clean_migration_files
        clear_migration_traps
        return 0
    fi

    # The exchange leaves the exact pathname entry displaced at publication in
    # migration_replacement. This closes the check/rename race: publish only if
    # that entry is still the inode and content that were transformed.
    displaced_identity=$(stat -c '%d:%i' -- "$migration_replacement" 2>/dev/null || true)
    if [ "$displaced_identity" != "$pinned_identity" ] ||
       ! cmp -s "$migration_replacement" "$migration_snapshot"; then
        echo "Warning: package config pathname changed during publication; restoring it untouched: $migration_file" >&2
        mark_migration_skipped

        published_identity=$(stat -c '%d:%i' -- "$migration_file" 2>/dev/null || true)
        if [ "$published_identity" != "$replacement_identity" ] ||
           ! python3 "$rename_exchange" "$migration_replacement" "$migration_file"; then
            recovery_file="$(dirname -- "$migration_file")/.$(basename -- "$migration_file").attune-recovery.$$.${RANDOM:-0}"
            if mv -fT -- "$migration_replacement" "$recovery_file"; then
                migration_replacement=
                echo "Error: a second concurrent pathname change prevented rollback; preserved the displaced administrator file at $recovery_file" >&2
            else
                echo "Error: a second concurrent pathname change prevented safe config rollback; recovery data remains in $migration_work_dir" >&2
                migration_work_dir=
            fi
            clear_migration_traps
            return 1
        fi

        restored_identity=$(stat -c '%d:%i' -- "$migration_file" 2>/dev/null || true)
        rollback_replacement_identity=$(stat -c '%d:%i' -- "$migration_replacement" 2>/dev/null || true)
        if [ "$restored_identity" != "$displaced_identity" ] ||
           [ "$rollback_replacement_identity" != "$replacement_identity" ]; then
            recovery_file="$(dirname -- "$migration_file")/.$(basename -- "$migration_file").attune-recovery.$$.${RANDOM:-0}"
            if mv -fT -- "$migration_replacement" "$recovery_file"; then
                migration_replacement=
                echo "Error: config rollback raced with another pathname update; preserved that update at $recovery_file" >&2
            else
                echo "Error: config rollback raced with another pathname update; recovery data remains in $migration_work_dir" >&2
                migration_work_dir=
            fi
            clear_migration_traps
            return 1
        fi

        sync -f "$migration_file"
        sync -f "$(dirname -- "$migration_file")"
        clean_migration_files
        clear_migration_traps
        return 0
    fi

    sync -f "$(dirname -- "$migration_file")"
    clean_migration_files
    clear_migration_traps
}

migrate_yaml_config() {
    prepare_status=0
    prepare_config_migration "$1" || prepare_status=$?
    [ "$prepare_status" -eq 2 ] && return 0
    [ "$prepare_status" -eq 0 ] || return "$prepare_status"

    if ! awk '
        function block_last_line(start,    i, last) {
            last = start
            for (i = start + 1; i <= line_count; i++) {
                if (lines[i] ~ /^[^[:space:]]/) {
                    break
                }
                if (lines[i] !~ /^[[:space:]]*$/) {
                    last = i
                }
            }
            return last
        }

        function obsolete_shipped_agent(start,    i, entries) {
            if (lines[start] !~ /^agent:[[:space:]]*$/) {
                return 0
            }

            entries = 0
            for (i = start + 1; i <= line_count; i++) {
                if (lines[i] ~ /^[^[:space:]]/) {
                    break
                }
                if (lines[i] ~ /^[[:space:]]*$/) {
                    continue
                }
                entries++
                if (lines[i] !~ /^  binary_dir: "\/var\/lib\/attune\/agent"[[:space:]]*$/) {
                    return 0
                }
            }
            return entries == 1
        }

        { lines[NR] = $0 }

        END {
            line_count = NR
            has_message_queue = 0
            for (i = 1; i <= line_count; i++) {
                if (lines[i] ~ /^message_queue:[[:space:]]*(#.*)?$/) {
                    has_message_queue = 1
                }
            }

            for (i = 1; i <= line_count; i++) {
                if (lines[i] == "# Agent binary directory (for serving agent downloads)" &&
                    obsolete_shipped_agent(i + 1)) {
                    i = block_last_line(i + 1)
                    continue
                }
                if (obsolete_shipped_agent(i)) {
                    i = block_last_line(i)
                    continue
                }
                if (lines[i] ~ /^rabbitmq:[[:space:]]*(#.*)?$/) {
                    if (!has_message_queue) {
                        sub(/^rabbitmq:/, "message_queue:", lines[i])
                        has_message_queue = 1
                        print lines[i]
                    } else {
                        last = block_last_line(i)
                        print "# Obsolete rabbitmq settings retained during package upgrade:"
                        for (j = i; j <= last; j++) {
                            print "# " lines[j]
                        }
                        i = last
                    }
                    continue
                }
                print lines[i]
            }
        }
    ' "$migration_snapshot" >"$migration_replacement"; then
        clean_migration_files
        clear_migration_traps
        return 1
    fi
    publish_config_migration
}

migrate_environment_config() {
    prepare_status=0
    prepare_config_migration "$1" || prepare_status=$?
    [ "$prepare_status" -eq 2 ] && return 0
    [ "$prepare_status" -eq 0 ] || return "$prepare_status"

    if ! awk '
        function is_active(line, name, pattern) {
            pattern = "^[[:space:]]*(export[[:space:]]+)?" name "[[:space:]]*="
            return line ~ pattern
        }

        function migrate(line, old_name, new_name, pattern) {
            pattern = "^[[:space:]]*#?[[:space:]]*(export[[:space:]]+)?" old_name "[[:space:]]*="
            if (line !~ pattern) {
                return line
            }
            if (is_active(line, old_name) && active[new_name]) {
                line = "# Migrated duplicate: " line
            }
            sub(old_name "[[:space:]]*=", new_name "=", line)
            return line
        }

        { lines[NR] = $0 }

        END {
            old[1] = "JWT_SECRET"
            new[1] = "ATTUNE__SECURITY__JWT_SECRET"
            old[2] = "ENCRYPTION_KEY"
            new[2] = "ATTUNE__SECURITY__ENCRYPTION_KEY"
            old[3] = "ATTUNE__RABBITMQ__URL"
            new[3] = "ATTUNE__MESSAGE_QUEUE__URL"

            for (i = 1; i <= NR; i++) {
                for (j = 1; j <= 3; j++) {
                    if (is_active(lines[i], new[j])) {
                        active[new[j]] = 1
                    }
                }
            }
            for (i = 1; i <= NR; i++) {
                for (j = 1; j <= 3; j++) {
                    lines[i] = migrate(lines[i], old[j], new[j])
                }
                print lines[i]
            }
        }
    ' "$migration_snapshot" >"$migration_replacement"; then
        clean_migration_files
        clear_migration_traps
        return 1
    fi
    publish_config_migration
}

# Package config files are noreplace, so remediate known obsolete shipped names
# in the administrator-owned copies after upgrades without replacing values.
migrate_yaml_config "$config_file"
migrate_environment_config "$environment_file"

if [ "${ATTUNE_PACKAGE_MIGRATE_ONLY:-0}" = 1 ]; then
    exit 0
fi

if [ "$migration_skipped" -eq 1 ]; then
    lifecycle=${ATTUNE_PACKAGE_SERVICE_LIFECYCLE:-/usr/lib/attune/package-hooks/service-lifecycle.sh}
    if [ -x "$lifecycle" ]; then
        sh "$lifecycle" block attune-api attune-executor attune-supervisor \
            attune-worker attune-sensor attune-notifier
    fi
fi

set_initial_config_permissions() {
    permissions_file=$1
    permissions_owner=$2
    permissions_parent=$(dirname -- "$permissions_file")
    permissions_name=$(basename -- "$permissions_file")
    permissions_work=$(mktemp -d "$permissions_parent/.${permissions_name}.permissions.XXXXXX") || return 1
    permissions_snapshot=$permissions_work/original

    if ! ln -P -- "$permissions_file" "$permissions_snapshot" 2>/dev/null; then
        rm -rf -- "$permissions_work"
        return 0
    fi
    if [ -L "$permissions_snapshot" ] || [ ! -f "$permissions_snapshot" ]; then
        rm -rf -- "$permissions_work"
        return 0
    fi

    # Mutate only the snapshotted regular inode, never a pathname that can be
    # exchanged for a symlink between a check and a privileged operation.
    chown "$permissions_owner" "$permissions_snapshot"
    chmod 0640 "$permissions_snapshot"
    rm -rf -- "$permissions_work"
}

# Create attune system user and group if they don't exist. Package extraction
# cannot resolve the config group on a fresh host, so remember that case.
attune_group_created=0
if ! getent group attune >/dev/null 2>&1; then
    groupadd --system attune
    attune_group_created=1
fi

attune_user_missing=0
if ! getent passwd attune >/dev/null 2>&1; then
    attune_user_missing=1
fi

if [ "$attune_group_created" -eq 1 ] || [ "$attune_user_missing" -eq 1 ]; then
    config_owner=${ATTUNE_PACKAGE_TEST_CONFIG_OWNER:-root:attune}
    for package_config in "$config_file" "$environment_file"; do
        set_initial_config_permissions "$package_config" "$config_owner"
    done
fi

if [ "$attune_user_missing" -eq 1 ]; then
    useradd --system --gid attune --home-dir /var/lib/attune \
        --shell /usr/sbin/nologin --comment "Attune automation platform" attune
fi

if [ "${ATTUNE_PACKAGE_TEST_ACCOUNT_ONLY:-0}" = 1 ]; then
    exit 0
fi

# Create required directories
for dir in /var/lib/attune /var/lib/attune/packs /var/lib/attune/runtime_envs \
           /var/lib/attune/artifacts /var/lib/attune/agent /var/log/attune; do
    mkdir -p "$dir"
    chown attune:attune "$dir"
    chmod 750 "$dir"
done

# The all-in-one installer keeps shipped binaries together under /opt.
mkdir -p /opt/attune-system
chown root:attune /opt/attune-system
chmod 755 /opt/attune-system

# Ensure config directory exists and has correct permissions
mkdir -p /etc/attune
chown root:attune /etc/attune
chmod 750 /etc/attune

if [ -x /opt/attune-system/attune-api ]; then
    migrate_cmd="set -a; . /etc/attune/environment; set +a; /opt/attune-system/attune-api --config /etc/attune/attune.yaml --migrate"
    service_set="attune-api attune-executor attune-supervisor attune-worker attune-sensor attune-notifier"
else
    migrate_cmd="set -a; . /etc/attune/environment; set +a; attune-api --config /etc/attune/attune.yaml --migrate"
    service_set="attune-api attune-executor attune-supervisor attune-notifier"
fi

# Reload systemd if a service unit was installed
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
fi

echo ""
echo "Attune installed successfully."
echo ""
echo "Next steps:"
echo "  1. Edit /etc/attune/environment to set ATTUNE__SECURITY__JWT_SECRET and ATTUNE__SECURITY__ENCRYPTION_KEY"
echo "  2. Set ATTUNE__MESSAGE_QUEUE__URL and configure the database URL"
echo "  3. Run database migrations: $migrate_cmd"
echo "  4. Enable and start services:"
echo "     systemctl enable --now $service_set"
echo ""
