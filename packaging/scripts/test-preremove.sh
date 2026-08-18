#!/bin/sh
set -eu

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
nfpm_dir=$script_dir/../nfpm
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT HUP INT TERM
mkdir "$test_dir/bin" "$test_dir/state" "$test_dir/service-state" \
    "$test_dir/installed-packages"

cat >"$test_dir/bin/dpkg-query" <<'EOF'
#!/bin/sh
for package_name do :; done
if [ -e "$FAKE_INSTALLED_PACKAGES/$package_name" ]; then
    printf 'install ok installed'
    exit 0
fi
exit 1
EOF
chmod +x "$test_dir/bin/dpkg-query"

cat >"$test_dir/bin/systemctl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$SYSTEMCTL_LOG"
for service do :; done
case "$1" in
    is-active) [ -e "$FAKE_SERVICE_STATE/$service.active" ] ;;
    is-enabled) [ -e "$FAKE_SERVICE_STATE/$service.enabled" ] ;;
    stop) rm -f "$FAKE_SERVICE_STATE/$service.active" ;;
    disable) rm -f "$FAKE_SERVICE_STATE/$service.enabled" ;;
    start) : >"$FAKE_SERVICE_STATE/$service.active" ;;
    enable) : >"$FAKE_SERVICE_STATE/$service.enabled" ;;
    try-restart) : ;;
    daemon-reload) : ;;
esac
EOF
chmod +x "$test_dir/bin/systemctl"

cat >"$test_dir/common-postinstall" <<'EOF'
#!/bin/sh
[ "${MIGRATION_SKIPPED:-0}" = 0 ] || printf 'skipped\n' >"$ATTUNE_PACKAGE_MIGRATION_STATUS_FILE"
EOF
chmod +x "$test_dir/common-postinstall"

cat >"$test_dir/all-in-one-links" <<'EOF'
#!/bin/sh
printf 'links %s\n' "$1" >>"$LINKS_LOG"
EOF
chmod +x "$test_dir/all-in-one-links"

run_hook() {
    PATH="$test_dir/bin:$PATH" \
    SYSTEMCTL_LOG="$test_dir/systemctl.log" \
    FAKE_SERVICE_STATE="$test_dir/service-state" \
    FAKE_INSTALLED_PACKAGES="$test_dir/installed-packages" \
    ATTUNE_PACKAGE_STATE_DIR="$test_dir/state" \
    ATTUNE_PACKAGE_CAPTURE_SERVICE_STATE="$script_dir/capture-service-state.sh" \
    ATTUNE_PACKAGE_SERVICE_LIFECYCLE="$script_dir/service-lifecycle.sh" \
    ATTUNE_PACKAGE_POSTINSTALL_SERVICE="$script_dir/postinstall-service.sh" \
    ATTUNE_PACKAGE_POSTINSTALL_COMMON="$test_dir/common-postinstall" \
    ATTUNE_PACKAGE_ALL_IN_ONE_LINKS="$test_dir/all-in-one-links" \
    LINKS_LOG="$test_dir/links.log" \
        sh "$script_dir/$1" ${2+"$2"} ${3+"$3"}
}

reset_test() {
    rm -f "$test_dir/state/"* "$test_dir/service-state/"* \
        "$test_dir/installed-packages/"*
    : >"$test_dir/systemctl.log"
    : >"$test_dir/links.log"
}

assert_service_removal() {
    reset_test
    service=$2
    : >"$test_dir/service-state/$service.active"
    : >"$test_dir/service-state/$service.enabled"
    run_hook "$1"
    [ ! -e "$test_dir/service-state/$service.active" ]
    [ ! -e "$test_dir/service-state/$service.enabled" ]
}

assert_service_removal preremove-attune-api.sh attune-api
assert_service_removal preremove-attune-executor.sh attune-executor
assert_service_removal preremove-attune-notifier.sh attune-notifier
assert_service_removal preremove-attune-supervisor.sh attune-supervisor

for hook in preremove.sh preremove-attune-api.sh preremove-attune-executor.sh \
            preremove-attune-notifier.sh preremove-attune-supervisor.sh; do
    reset_test
    run_hook "$hook" upgrade
    [ ! -s "$test_dir/systemctl.log" ]
    run_hook "$hook" 1
    [ ! -s "$test_dir/systemctl.log" ]
done

# Debian installs/configures the new shared dependency before upgrading the
# legacy component. Its self-contained preinst records state before old-prerm.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preinstall-attune-common.sh install
run_hook preremove-attune-api.sh
run_hook postinstall-attune-api.sh configure 0.2.1
[ -e "$test_dir/service-state/attune-api.active" ]
[ -e "$test_dir/service-state/attune-api.enabled" ]
grep -q '^start attune-api$' "$test_dir/systemctl.log"
grep -q '^enable attune-api$' "$test_dir/systemctl.log"

# Incoming preinstall can also identify the outgoing layout before its removal
# hook runs, as happens in RPM replacement transactions.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
: >"$test_dir/installed-packages/attune-api"
run_hook preinstall-attune.sh 1
rm -f "$test_dir/installed-packages/attune-api"
run_hook preremove-attune-api.sh remove
run_hook postinstall-attune.sh configure
[ -e "$test_dir/service-state/attune-api.active" ]
[ -e "$test_dir/service-state/attune-api.enabled" ]
grep -q '^start attune-api$' "$test_dir/systemctl.log"
grep -q '^enable attune-api$' "$test_dir/systemctl.log"

# Captured active/enabled dimensions are restored independently.
reset_test
: >"$test_dir/service-state/attune-api.enabled"
run_hook preinstall-attune-common.sh install
run_hook preremove-attune-api.sh
run_hook postinstall-attune-api.sh configure 0.2.1
[ ! -e "$test_dir/service-state/attune-api.active" ]
[ -e "$test_dir/service-state/attune-api.enabled" ]
if grep -q '^start attune-api$' "$test_dir/systemctl.log"; then exit 1; fi

reset_test
: >"$test_dir/service-state/attune-api.active"
run_hook preinstall-attune-common.sh install
run_hook preremove-attune-api.sh
run_hook postinstall-attune-api.sh configure 0.2.1
[ -e "$test_dir/service-state/attune-api.active" ]
[ ! -e "$test_dir/service-state/attune-api.enabled" ]
if grep -q '^enable attune-api$' "$test_dir/systemctl.log"; then exit 1; fi

# RPM must wait for posttrans because legacy preun runs after the new post.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preinstall-attune-api.sh 2
run_hook postinstall-attune-api.sh 2
if grep -q '^try-restart attune-api$' "$test_dir/systemctl.log"; then
    echo "RPM postinstall restarted before posttrans" >&2
    exit 1
fi
run_hook preremove-attune-api.sh
[ ! -e "$test_dir/service-state/attune-api.active" ]
run_hook postupgrade-attune-api.sh 0
[ -e "$test_dir/service-state/attune-api.active" ]
[ -e "$test_dir/service-state/attune-api.enabled" ]

# A normal RPM upgrade restarts exactly once, from posttrans.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preinstall-attune-api.sh 2
run_hook postinstall-attune-api.sh 2
run_hook preremove-attune-api.sh 1
run_hook postupgrade-attune-api.sh 0
[ "$(grep -c '^try-restart attune-api$' "$test_dir/systemctl.log")" -eq 1 ]

# Arch's new pre_upgrade receives two versions and recovers in post_upgrade.
reset_test
: >"$test_dir/service-state/attune-api.active"
run_hook preinstall-attune-api.sh 0.3.0-1 0.2.1-1
rm -f "$test_dir/service-state/attune-api.active"
run_hook postupgrade-attune-api.sh 0.3.0-1 0.2.1-1
[ -e "$test_dir/service-state/attune-api.active" ]

# Fresh RPM install explicitly clears stale capture and try-restart never starts.
reset_test
: >"$test_dir/state/attune-api.active"
: >"$test_dir/state/attune-api.enabled"
run_hook preinstall-attune-api.sh 1
run_hook postinstall-attune-api.sh 1
[ ! -e "$test_dir/service-state/attune-api.active" ]
[ ! -e "$test_dir/service-state/attune-api.enabled" ]
if grep -q '^start attune-api\|^enable attune-api' "$test_dir/systemctl.log"; then exit 1; fi

# Replacing a split package with all-in-one preserves state even though the
# incoming package manager action is a fresh install.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preremove-attune-api.sh remove
run_hook preinstall-attune.sh install
run_hook postinstall-attune.sh configure
[ -e "$test_dir/service-state/attune-api.active" ]
[ -e "$test_dir/service-state/attune-api.enabled" ]
grep -q '^start attune-api$' "$test_dir/systemctl.log"
grep -q '^enable attune-api$' "$test_dir/systemctl.log"

# Replacing all-in-one with a split package preserves only recorded service
# state and does not require a same-package old-version argument.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preremove.sh remove
run_hook preinstall-attune-api.sh install
run_hook postinstall-attune-api.sh configure
[ -e "$test_dir/service-state/attune-api.active" ]
[ -e "$test_dir/service-state/attune-api.enabled" ]
grep -q '^start attune-api$' "$test_dir/systemctl.log"
grep -q '^enable attune-api$' "$test_dir/systemctl.log"

# A clean fresh install has no bridge marker and must not start or enable.
reset_test
run_hook preinstall-attune-api.sh install
run_hook postinstall-attune-api.sh configure
if grep -q '^start attune-api\|^enable attune-api\|^try-restart attune-api' "$test_dir/systemctl.log"; then
    echo "unrelated fresh install changed service state" >&2
    exit 1
fi

# Removing and freshly reinstalling the same layout is not a bridge and must
# not resurrect the state captured by the removal hook.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preremove-attune-api.sh remove
run_hook preinstall-attune-api.sh install
run_hook postinstall-attune-api.sh configure
if grep -q '^start attune-api\|^enable attune-api\|^try-restart attune-api' "$test_dir/systemctl.log"; then
    echo "same-layout fresh reinstall restored removed service state" >&2
    exit 1
fi

# A skipped symlink migration blocks both immediate and posttrans recovery.
reset_test
: >"$test_dir/service-state/attune-api.active"
: >"$test_dir/service-state/attune-api.enabled"
run_hook preinstall-attune-api.sh 2
MIGRATION_SKIPPED=1 run_hook postinstall-attune-api.sh 2
if grep -q '^try-restart attune-api$' "$test_dir/systemctl.log"; then
    echo "RPM postinstall restarted after a skipped migration" >&2
    exit 1
fi
run_hook preremove-attune-api.sh
MIGRATION_SKIPPED=1 run_hook postupgrade-attune-api.sh 0 2>"$test_dir/blocked.stderr"
[ ! -e "$test_dir/service-state/attune-api.active" ]
[ ! -e "$test_dir/service-state/attune-api.enabled" ]
grep -q 'not restarting attune-api because automatic configuration migration was skipped' "$test_dir/blocked.stderr"

# Verify format-specific ordering and sole shared-file ownership in nFPM input.
for config in attune.yaml attune-api.yaml attune-executor.yaml attune-notifier.yaml attune-supervisor.yaml; do
    grep -Fq 'attune-common' "$nfpm_dir/$config"
    if grep -Fq 'dst: /etc/attune/' "$nfpm_dir/$config"; then
        echo "$config still owns shared configuration" >&2
        exit 1
    fi
    grep -Fq 'preinstall:' "$nfpm_dir/$config"
    grep -Fq 'posttrans:' "$nfpm_dir/$config"
    grep -Fq 'preupgrade:' "$nfpm_dir/$config"
    grep -Fq 'postupgrade:' "$nfpm_dir/$config"
done
grep -Fq 'dst: /etc/attune/attune.yaml' "$nfpm_dir/attune-common.yaml"
grep -Fq 'dst: /etc/attune/environment' "$nfpm_dir/attune-common.yaml"
grep -Fq 'dst: /usr/lib/attune/package-hooks/postinstall-common.sh' "$nfpm_dir/attune-common.yaml"
grep -Fq 'dst: /usr/lib/attune/package-hooks/all-in-one-links.sh' "$nfpm_dir/attune-common.yaml"
if grep -Fq 'dst: /var/lib/attune/agent/' "$nfpm_dir/attune.yaml"; then
    echo "all-in-one package still owns shared agent state" >&2
    exit 1
fi

# Hook-managed links leave shared directories in place and preserve unrelated
# administrator-managed entries during install and removal.
mkdir "$test_dir/link-data" "$test_dir/link-agent" "$test_dir/link-opt"
: >"$test_dir/link-agent/attune-mcp"
ATTUNE_PACKAGE_DATA_DIR="$test_dir/link-data" \
ATTUNE_PACKAGE_AGENT_DIR="$test_dir/link-agent" \
ATTUNE_PACKAGE_OPT_DIR="$test_dir/link-opt" \
ATTUNE_PACKAGE_DATA_OWNER="$(id -u):$(id -g)" \
ATTUNE_PACKAGE_OPT_OWNER="$(id -u):$(id -g)" \
    sh "$script_dir/all-in-one-links.sh" install 2>"$test_dir/links.stderr"
[ -L "$test_dir/link-agent/attune" ]
[ ! -L "$test_dir/link-agent/attune-mcp" ]
grep -q 'not replacing administrator-managed agent path' "$test_dir/links.stderr"
[ "$(stat -c '%a' "$test_dir/link-data")" = 750 ]
[ "$(stat -c '%a' "$test_dir/link-agent")" = 750 ]
ATTUNE_PACKAGE_DATA_DIR="$test_dir/link-data" \
ATTUNE_PACKAGE_AGENT_DIR="$test_dir/link-agent" \
ATTUNE_PACKAGE_OPT_DIR="$test_dir/link-opt" \
    sh "$script_dir/all-in-one-links.sh" remove
[ -d "$test_dir/link-agent" ]
[ -f "$test_dir/link-agent/attune-mcp" ]
[ ! -e "$test_dir/link-agent/attune" ]

echo "package lifecycle tests passed"
