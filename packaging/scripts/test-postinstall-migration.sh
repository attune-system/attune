#!/bin/sh
set -eu

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
postinstall=$script_dir/postinstall.sh
ATTUNE_PACKAGE_RENAME_EXCHANGE=$script_dir/rename-exchange.py
export ATTUNE_PACKAGE_RENAME_EXCHANGE
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT HUP INT TERM

metadata() {
    stat -c '%u:%g:%a' "$1"
}

assert_metadata() {
    actual=$(metadata "$1")
    if [ "$actual" != "$2" ]; then
        echo "metadata changed for $1: expected $2, got $actual" >&2
        exit 1
    fi
}

cat >"$test_dir/attune.yaml" <<'EOF'
database:
  url: "postgresql://custom"
rabbitmq:
  url: "amqp://custom"
  connection_timeout: 30

# Agent binary directory (for serving agent downloads)
agent:
  binary_dir: "/var/lib/attune/agent"

custom_setting: true
EOF

cat >"$test_dir/environment" <<'EOF'
JWT_SECRET=custom-jwt
export ENCRYPTION_KEY=custom-encryption
# ATTUNE__RABBITMQ__URL=amqp://commented
ATTUNE__RABBITMQ__URL=amqp://custom
CUSTOM_VALUE=preserved
EOF

ln "$test_dir/attune.yaml" "$test_dir/attune.yaml.hardlink"
cp "$test_dir/attune.yaml.hardlink" "$test_dir/attune.yaml.hardlink.original"
chmod 0641 "$test_dir/attune.yaml"
chmod 0604 "$test_dir/environment"

acl_supported=0
if command -v setfacl >/dev/null 2>&1 && command -v getfacl >/dev/null 2>&1 &&
   setfacl -m u:65534:r-- "$test_dir/attune.yaml" 2>/dev/null; then
    acl_supported=1
    yaml_acl=$(getfacl -cp "$test_dir/attune.yaml")
fi

xattr_supported=0
if command -v setfattr >/dev/null 2>&1 && command -v getfattr >/dev/null 2>&1 &&
   setfattr -n user.attune-migration -v preserved "$test_dir/attune.yaml" 2>/dev/null; then
    xattr_supported=1
    yaml_xattr=$(getfattr --absolute-names --only-values -n user.attune-migration "$test_dir/attune.yaml")
fi

selinux_supported=0
if yaml_selinux=$(stat -c '%C' "$test_dir/attune.yaml" 2>/dev/null); then
    selinux_supported=1
fi

yaml_metadata=$(metadata "$test_dir/attune.yaml")
environment_metadata=$(metadata "$test_dir/environment")
changed_yaml_inode=$(stat -c '%i' "$test_dir/attune.yaml")
changed_environment_inode=$(stat -c '%i' "$test_dir/environment")

run_migration() {
    ATTUNE_PACKAGE_CONFIG_FILE="${1:-$test_dir/attune.yaml}" \
    ATTUNE_PACKAGE_ENVIRONMENT_FILE="${2:-$test_dir/environment}" \
    ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
        sh "$postinstall"
}

run_migration

grep -q '^message_queue:$' "$test_dir/attune.yaml"
grep -q '^  connection_timeout: 30$' "$test_dir/attune.yaml"
grep -q '^custom_setting: true$' "$test_dir/attune.yaml"
if grep -q '^rabbitmq:\|^agent:' "$test_dir/attune.yaml"; then
    echo "obsolete YAML configuration was not removed" >&2
    exit 1
fi

grep -q '^ATTUNE__SECURITY__JWT_SECRET=custom-jwt$' "$test_dir/environment"
grep -q '^export ATTUNE__SECURITY__ENCRYPTION_KEY=custom-encryption$' "$test_dir/environment"
grep -q '^ATTUNE__MESSAGE_QUEUE__URL=amqp://custom$' "$test_dir/environment"
grep -q '^# ATTUNE__MESSAGE_QUEUE__URL=amqp://commented$' "$test_dir/environment"
grep -q '^CUSTOM_VALUE=preserved$' "$test_dir/environment"
[ "$(stat -c '%i' "$test_dir/attune.yaml")" != "$changed_yaml_inode" ]
[ "$(stat -c '%i' "$test_dir/attune.yaml.hardlink")" = "$changed_yaml_inode" ]
cmp -s "$test_dir/attune.yaml.hardlink.original" "$test_dir/attune.yaml.hardlink"
[ "$(stat -c '%i' "$test_dir/environment")" != "$changed_environment_inode" ]
assert_metadata "$test_dir/attune.yaml" "$yaml_metadata"
assert_metadata "$test_dir/environment" "$environment_metadata"
if [ "$acl_supported" -eq 1 ]; then
    [ "$(getfacl -cp "$test_dir/attune.yaml")" = "$yaml_acl" ]
fi
if [ "$xattr_supported" -eq 1 ]; then
    [ "$(getfattr --absolute-names --only-values -n user.attune-migration "$test_dir/attune.yaml")" = "$yaml_xattr" ]
fi
if [ "$selinux_supported" -eq 1 ]; then
    [ "$(stat -c '%C' "$test_dir/attune.yaml")" = "$yaml_selinux" ]
fi

cp "$test_dir/attune.yaml" "$test_dir/attune.yaml.once"
cp "$test_dir/environment" "$test_dir/environment.once"
yaml_inode=$(stat -c '%i' "$test_dir/attune.yaml")
environment_inode=$(stat -c '%i' "$test_dir/environment")
run_migration
cmp -s "$test_dir/attune.yaml.once" "$test_dir/attune.yaml"
cmp -s "$test_dir/environment.once" "$test_dir/environment"
[ "$(stat -c '%i' "$test_dir/attune.yaml")" = "$yaml_inode" ]
[ "$(stat -c '%i' "$test_dir/environment")" = "$environment_inode" ]
assert_metadata "$test_dir/attune.yaml" "$yaml_metadata"
assert_metadata "$test_dir/environment" "$environment_metadata"

cat >"$test_dir/attune.yaml" <<'EOF'
message_queue:
  url: "amqp://canonical"
rabbitmq:
  url: "amqp://legacy-custom"

agent:
  binary_dir: "/var/lib/attune/agent"
  bootstrap_token: "administrator-token"
EOF

cat >"$test_dir/environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=canonical
JWT_SECRET=legacy-custom
EOF

run_migration

grep -q '^  url: "amqp://canonical"$' "$test_dir/attune.yaml"
grep -q '^# rabbitmq:$' "$test_dir/attune.yaml"
grep -q '^#   url: "amqp://legacy-custom"$' "$test_dir/attune.yaml"
grep -q '^agent:$' "$test_dir/attune.yaml"
grep -q '^  bootstrap_token: "administrator-token"$' "$test_dir/attune.yaml"
grep -q '^ATTUNE__SECURITY__JWT_SECRET=canonical$' "$test_dir/environment"
grep -q '^# Migrated duplicate: ATTUNE__SECURITY__JWT_SECRET=legacy-custom$' "$test_dir/environment"

mkdir "$test_dir/targets"
cat >"$test_dir/targets/linked.yaml" <<'EOF'
rabbitmq:
  url: "amqp://linked"
EOF
cat >"$test_dir/targets/linked-environment" <<'EOF'
JWT_SECRET=linked-jwt
EOF
chmod 0644 "$test_dir/targets/linked.yaml"
chmod 0600 "$test_dir/targets/linked-environment"
linked_yaml_metadata=$(metadata "$test_dir/targets/linked.yaml")
linked_environment_metadata=$(metadata "$test_dir/targets/linked-environment")

# Exercise both relative and absolute administrator-managed links.
ln -s targets/linked.yaml "$test_dir/linked.yaml"
ln -s "$test_dir/targets/linked-environment" "$test_dir/linked-environment"
yaml_link=$(readlink "$test_dir/linked.yaml")
environment_link=$(readlink "$test_dir/linked-environment")
changed_linked_yaml_inode=$(stat -c '%i' "$test_dir/targets/linked.yaml")
changed_linked_environment_inode=$(stat -c '%i' "$test_dir/targets/linked-environment")

run_migration "$test_dir/linked.yaml" "$test_dir/linked-environment" \
    2>"$test_dir/symlink.stderr"

[ -L "$test_dir/linked.yaml" ]
[ -L "$test_dir/linked-environment" ]
[ "$(readlink "$test_dir/linked.yaml")" = "$yaml_link" ]
[ "$(readlink "$test_dir/linked-environment")" = "$environment_link" ]
[ "$(stat -c '%i' "$test_dir/targets/linked.yaml")" = "$changed_linked_yaml_inode" ]
[ "$(stat -c '%i' "$test_dir/targets/linked-environment")" = "$changed_linked_environment_inode" ]
grep -q '^rabbitmq:$' "$test_dir/targets/linked.yaml"
grep -q '^JWT_SECRET=linked-jwt$' "$test_dir/targets/linked-environment"
grep -Fq "administrator-managed symbolic link: $test_dir/linked.yaml" "$test_dir/symlink.stderr"
grep -Fq "administrator-managed symbolic link: $test_dir/linked-environment" "$test_dir/symlink.stderr"
assert_metadata "$test_dir/targets/linked.yaml" "$linked_yaml_metadata"
assert_metadata "$test_dir/targets/linked-environment" "$linked_environment_metadata"

cp "$test_dir/targets/linked.yaml" "$test_dir/linked.yaml.once"
cp "$test_dir/targets/linked-environment" "$test_dir/linked-environment.once"
linked_yaml_inode=$(stat -c '%i' "$test_dir/targets/linked.yaml")
linked_environment_inode=$(stat -c '%i' "$test_dir/targets/linked-environment")
run_migration "$test_dir/linked.yaml" "$test_dir/linked-environment"
cmp -s "$test_dir/linked.yaml.once" "$test_dir/targets/linked.yaml"
cmp -s "$test_dir/linked-environment.once" "$test_dir/targets/linked-environment"
[ "$(stat -c '%i' "$test_dir/targets/linked.yaml")" = "$linked_yaml_inode" ]
[ "$(stat -c '%i' "$test_dir/targets/linked-environment")" = "$linked_environment_inode" ]
assert_metadata "$test_dir/targets/linked.yaml" "$linked_yaml_metadata"
assert_metadata "$test_dir/targets/linked-environment" "$linked_environment_metadata"

# Simulate replacing a regular config with a symlink exactly when the migration
# snapshots it. ln -P must capture the link itself rather than follow its target.
mkdir "$test_dir/race-bin"
cat >"$test_dir/race-bin/ln" <<'EOF'
#!/bin/sh
if [ "$3" = "$RACE_PATH" ] && [ ! -e "$RACE_ORIGINAL" ]; then
    /bin/mv -- "$RACE_PATH" "$RACE_ORIGINAL"
    /bin/ln -s -- "$RACE_VICTIM" "$RACE_PATH"
fi
exec /bin/ln "$@"
EOF
chmod +x "$test_dir/race-bin/ln"
cat >"$test_dir/race.yaml" <<'EOF'
rabbitmq:
  url: "amqp://race-original"
EOF
cat >"$test_dir/race-victim.yaml" <<'EOF'
rabbitmq:
  url: "amqp://must-not-change"
EOF
cat >"$test_dir/race-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=unchanged
EOF
cp "$test_dir/race.yaml" "$test_dir/race.yaml.expected"
cp "$test_dir/race-victim.yaml" "$test_dir/race-victim.yaml.expected"
RACE_PATH="$test_dir/race.yaml" \
RACE_ORIGINAL="$test_dir/race-original.yaml" \
RACE_VICTIM="$test_dir/race-victim.yaml" \
ATTUNE_PACKAGE_CONFIG_FILE="$test_dir/race.yaml" \
ATTUNE_PACKAGE_ENVIRONMENT_FILE="$test_dir/race-environment" \
ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
PATH="$test_dir/race-bin:$PATH" \
    sh "$postinstall" 2>"$test_dir/race.stderr"
[ -L "$test_dir/race.yaml" ]
cmp -s "$test_dir/race.yaml.expected" "$test_dir/race-original.yaml"
cmp -s "$test_dir/race-victim.yaml.expected" "$test_dir/race-victim.yaml"
grep -Fq "administrator-managed symbolic link: $test_dir/race.yaml" "$test_dir/race.stderr"

mkdir "$test_dir/in-place-bin"
cat >"$test_dir/in-place-bin/sync" <<'EOF'
#!/bin/sh
if [ ! -e "$IN_PLACE_WRITE_DONE" ]; then
    printf 'message_queue:\n  url: "amqp://concurrent"\n' >"$IN_PLACE_WRITE_PATH"
    : >"$IN_PLACE_WRITE_DONE"
fi
exec /bin/sync "$@"
EOF
chmod +x "$test_dir/in-place-bin/sync"
cat >"$test_dir/in-place.yaml" <<'EOF'
rabbitmq:
  url: "amqp://before-race"
EOF
cat >"$test_dir/in-place-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=unchanged
EOF
IN_PLACE_WRITE_PATH="$test_dir/in-place.yaml" \
IN_PLACE_WRITE_DONE="$test_dir/in-place-write.done" \
ATTUNE_PACKAGE_CONFIG_FILE="$test_dir/in-place.yaml" \
ATTUNE_PACKAGE_ENVIRONMENT_FILE="$test_dir/in-place-environment" \
ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
PATH="$test_dir/in-place-bin:$PATH" \
    sh "$postinstall" 2>"$test_dir/in-place.stderr"
grep -q '^message_queue:$' "$test_dir/in-place.yaml"
grep -q '^  url: "amqp://concurrent"$' "$test_dir/in-place.yaml"
grep -Fq "changed during migration; leaving it untouched: $test_dir/in-place.yaml" \
    "$test_dir/in-place.stderr"
if ls -d "$test_dir/.in-place.yaml.migrate."* >/dev/null 2>&1; then
    echo "migration artifact remained after an in-place race" >&2
    exit 1
fi

# Replace the pathname after the final shell-level identity check. Atomic
# exchange must expose the displaced unexpected inode and roll it back.
mkdir "$test_dir/publish-race-bin"
cat >"$test_dir/publish-race-bin/rename-exchange.py" <<'EOF'
import os
import sys

target = sys.argv[2]
if not os.path.exists(os.environ["PUBLISH_RACE_DONE"]):
    os.rename(target, os.environ["PUBLISH_RACE_ORIGINAL"])
    with open(target, "w", encoding="utf-8") as stream:
        stream.write('message_queue:\n  url: "amqp://publish-concurrent"\n')
    open(os.environ["PUBLISH_RACE_DONE"], "w", encoding="utf-8").close()
os.execv(sys.executable, [sys.executable, os.environ["REAL_RENAME_EXCHANGE"], *sys.argv[1:]])
EOF
cat >"$test_dir/publish-race.yaml" <<'EOF'
rabbitmq:
  url: "amqp://before-publication"
EOF
cat >"$test_dir/publish-race-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=unchanged
EOF
PUBLISH_RACE_PATH="$test_dir/publish-race.yaml" \
PUBLISH_RACE_DONE="$test_dir/publish-race.done" \
PUBLISH_RACE_ORIGINAL="$test_dir/publish-race.original" \
REAL_RENAME_EXCHANGE="$script_dir/rename-exchange.py" \
ATTUNE_PACKAGE_RENAME_EXCHANGE="$test_dir/publish-race-bin/rename-exchange.py" \
ATTUNE_PACKAGE_CONFIG_FILE="$test_dir/publish-race.yaml" \
ATTUNE_PACKAGE_ENVIRONMENT_FILE="$test_dir/publish-race-environment" \
ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
    sh "$postinstall" 2>"$test_dir/publish-race.stderr"
grep -q '^message_queue:$' "$test_dir/publish-race.yaml"
grep -q '^  url: "amqp://publish-concurrent"$' "$test_dir/publish-race.yaml"
grep -Fq "pathname changed during publication; restoring it untouched: $test_dir/publish-race.yaml" \
    "$test_dir/publish-race.stderr"
if ls -d "$test_dir/.publish-race.yaml.migrate."* >/dev/null 2>&1; then
    echo "migration artifact remained after a publication race" >&2
    exit 1
fi

mkdir "$test_dir/account-bin"
cat >"$test_dir/account-bin/getent" <<'EOF'
#!/bin/sh
case "$1:$2" in
    group:attune) [ "$ATTUNE_TEST_GROUP_EXISTS" = 1 ] ;;
    passwd:attune) [ "$ATTUNE_TEST_PASSWD_EXISTS" = 1 ] ;;
    *) exit 1 ;;
esac
EOF
cat >"$test_dir/account-bin/groupadd" <<'EOF'
#!/bin/sh
printf 'groupadd %s\n' "$*" >>"$ATTUNE_TEST_ACCOUNT_LOG"
EOF
cat >"$test_dir/account-bin/useradd" <<'EOF'
#!/bin/sh
printf 'config-at-useradd %s %s\n' \
    "$(stat -c '%u:%g:%a' "$ATTUNE_PACKAGE_CONFIG_FILE")" \
    "$(stat -c '%u:%g:%a' "$ATTUNE_PACKAGE_ENVIRONMENT_FILE")" \
    >>"$ATTUNE_TEST_ACCOUNT_LOG"
printf 'useradd %s\n' "$*" >>"$ATTUNE_TEST_ACCOUNT_LOG"
[ "${ATTUNE_TEST_USERADD_FAIL:-0}" != 1 ]
EOF
chmod +x "$test_dir/account-bin/getent" \
         "$test_dir/account-bin/groupadd" \
         "$test_dir/account-bin/useradd"

run_account_setup() {
    : >"$test_dir/account.log"
    ATTUNE_PACKAGE_CONFIG_FILE=$1 \
    ATTUNE_PACKAGE_ENVIRONMENT_FILE=$2 \
    ATTUNE_PACKAGE_TEST_CONFIG_OWNER="$(id -u):$(id -g)" \
    ATTUNE_PACKAGE_TEST_ACCOUNT_ONLY=1 \
    ATTUNE_TEST_GROUP_EXISTS=$3 \
    ATTUNE_TEST_PASSWD_EXISTS=$4 \
    ATTUNE_TEST_USERADD_FAIL=${5:-0} \
    ATTUNE_TEST_ACCOUNT_LOG="$test_dir/account.log" \
    PATH="$test_dir/account-bin:$PATH" \
        sh "$postinstall"
}

cat >"$test_dir/fresh.yaml" <<'EOF'
message_queue:
  url: "amqp://fresh"
EOF
cat >"$test_dir/fresh-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=fresh
EOF
chmod 0600 "$test_dir/fresh.yaml" "$test_dir/fresh-environment"
run_account_setup "$test_dir/fresh.yaml" "$test_dir/fresh-environment" 0 0
expected_fresh_metadata="$(id -u):$(id -g):640"
assert_metadata "$test_dir/fresh.yaml" "$expected_fresh_metadata"
assert_metadata "$test_dir/fresh-environment" "$expected_fresh_metadata"
grep -q '^groupadd --system attune$' "$test_dir/account.log"
grep -q "^config-at-useradd $expected_fresh_metadata $expected_fresh_metadata$" "$test_dir/account.log"
grep -q '^useradd --system --gid attune .* attune$' "$test_dir/account.log"

cat >"$test_dir/retry.yaml" <<'EOF'
message_queue:
  url: "amqp://retry"
EOF
cat >"$test_dir/retry-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=retry
EOF
chmod 0600 "$test_dir/retry.yaml" "$test_dir/retry-environment"
if run_account_setup "$test_dir/retry.yaml" "$test_dir/retry-environment" 0 0 1; then
    echo "account setup unexpectedly succeeded with a failing useradd" >&2
    exit 1
fi
assert_metadata "$test_dir/retry.yaml" "$expected_fresh_metadata"
assert_metadata "$test_dir/retry-environment" "$expected_fresh_metadata"

# A retry can see the group from the first attempt while the user is missing.
chmod 0600 "$test_dir/retry.yaml" "$test_dir/retry-environment"
run_account_setup "$test_dir/retry.yaml" "$test_dir/retry-environment" 1 0
assert_metadata "$test_dir/retry.yaml" "$expected_fresh_metadata"
assert_metadata "$test_dir/retry-environment" "$expected_fresh_metadata"
grep -q "^config-at-useradd $expected_fresh_metadata $expected_fresh_metadata$" "$test_dir/account.log"
if grep -q '^groupadd ' "$test_dir/account.log"; then
    echo "retry attempted to recreate the existing attune group" >&2
    exit 1
fi

cat >"$test_dir/upgrade.yaml" <<'EOF'
message_queue:
  url: "amqp://upgrade"
EOF
cat >"$test_dir/upgrade-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=upgrade
EOF
chmod 0604 "$test_dir/upgrade.yaml"
chmod 0644 "$test_dir/upgrade-environment"
upgrade_yaml_metadata=$(metadata "$test_dir/upgrade.yaml")
upgrade_environment_metadata=$(metadata "$test_dir/upgrade-environment")
run_account_setup "$test_dir/upgrade.yaml" "$test_dir/upgrade-environment" 1 1
assert_metadata "$test_dir/upgrade.yaml" "$upgrade_yaml_metadata"
assert_metadata "$test_dir/upgrade-environment" "$upgrade_environment_metadata"
[ ! -s "$test_dir/account.log" ]

cat >"$test_dir/targets/account-linked.yaml" <<'EOF'
message_queue:
  url: "amqp://linked-fresh"
EOF
cat >"$test_dir/targets/account-linked-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=linked-fresh
EOF
chmod 0600 "$test_dir/targets/account-linked.yaml"
chmod 0604 "$test_dir/targets/account-linked-environment"
ln -s targets/account-linked.yaml "$test_dir/account-linked.yaml"
ln -s "$test_dir/targets/account-linked-environment" "$test_dir/account-linked-environment"
account_linked_yaml_metadata=$(metadata "$test_dir/targets/account-linked.yaml")
account_linked_environment_metadata=$(metadata "$test_dir/targets/account-linked-environment")
run_account_setup "$test_dir/account-linked.yaml" "$test_dir/account-linked-environment" 0 0
assert_metadata "$test_dir/targets/account-linked.yaml" "$account_linked_yaml_metadata"
assert_metadata "$test_dir/targets/account-linked-environment" "$account_linked_environment_metadata"
[ -L "$test_dir/account-linked.yaml" ]
[ -L "$test_dir/account-linked-environment" ]

mkdir "$test_dir/failing-bin"
cat >"$test_dir/failing-bin/awk" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "$test_dir/failing-bin/awk"
cat >"$test_dir/write-error.yaml" <<'EOF'
rabbitmq:
  url: "amqp://write-error"
EOF
cat >"$test_dir/write-error-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=unchanged
EOF
cp "$test_dir/write-error.yaml" "$test_dir/write-error.yaml.original"
write_error_inode=$(stat -c '%i' "$test_dir/write-error.yaml")
if ATTUNE_PACKAGE_CONFIG_FILE="$test_dir/write-error.yaml" \
   ATTUNE_PACKAGE_ENVIRONMENT_FILE="$test_dir/write-error-environment" \
   ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
   PATH="$test_dir/failing-bin:$PATH" \
       sh "$postinstall"; then
    echo "migration unexpectedly succeeded after a target write error" >&2
    exit 1
fi
cmp -s "$test_dir/write-error.yaml.original" "$test_dir/write-error.yaml"
[ "$(stat -c '%i' "$test_dir/write-error.yaml")" = "$write_error_inode" ]
if ls -d "$test_dir/.write-error.yaml.migrate."* >/dev/null 2>&1; then
    echo "migration artifact remained after a target write error" >&2
    exit 1
fi

mkdir "$test_dir/publish-error-bin"
cat >"$test_dir/publish-error-bin/touch" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "$test_dir/publish-error-bin/touch"
cat >"$test_dir/publish-error.yaml" <<'EOF'
rabbitmq:
  url: "amqp://publish-error"
EOF
cat >"$test_dir/publish-error-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=unchanged
EOF
cp "$test_dir/publish-error.yaml" "$test_dir/publish-error.yaml.original"
if ATTUNE_PACKAGE_CONFIG_FILE="$test_dir/publish-error.yaml" \
   ATTUNE_PACKAGE_ENVIRONMENT_FILE="$test_dir/publish-error-environment" \
   ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
   PATH="$test_dir/publish-error-bin:$PATH" \
       sh "$postinstall"; then
    echo "migration unexpectedly succeeded after a publication setup error" >&2
    exit 1
fi
cmp -s "$test_dir/publish-error.yaml.original" "$test_dir/publish-error.yaml"
if ls -d "$test_dir/.publish-error.yaml.migrate."* >/dev/null 2>&1; then
    echo "EXIT cleanup left an artifact after a publication setup error" >&2
    exit 1
fi

mkdir "$test_dir/signal-bin"
cat >"$test_dir/signal-bin/sync" <<'EOF'
#!/bin/sh
kill -KILL "$PPID"
sleep 1
EOF
chmod +x "$test_dir/signal-bin/sync"
cat >"$test_dir/signal.yaml" <<'EOF'
rabbitmq:
  url: "amqp://signal"
EOF
cat >"$test_dir/signal-environment" <<'EOF'
ATTUNE__SECURITY__JWT_SECRET=unchanged
EOF
cp "$test_dir/signal.yaml" "$test_dir/signal.yaml.original"
signal_inode=$(stat -c '%i' "$test_dir/signal.yaml")
if ATTUNE_PACKAGE_CONFIG_FILE="$test_dir/signal.yaml" \
   ATTUNE_PACKAGE_ENVIRONMENT_FILE="$test_dir/signal-environment" \
   ATTUNE_PACKAGE_MIGRATE_ONLY=1 \
   PATH="$test_dir/signal-bin:$PATH" \
       sh "$postinstall"; then
    echo "migration unexpectedly succeeded after SIGKILL" >&2
    exit 1
fi
cmp -s "$test_dir/signal.yaml.original" "$test_dir/signal.yaml"
[ "$(stat -c '%i' "$test_dir/signal.yaml")" = "$signal_inode" ]
if ! ls -d "$test_dir/.signal.yaml.migrate."* >/dev/null 2>&1; then
    echo "SIGKILL did not interrupt migration before atomic publication" >&2
    exit 1
fi

# Publication is an fsync followed by a same-directory atomic exchange. The
# displaced pathname is verified before the administrator's old entry is freed.
grep -Fq 'sync -f "$migration_replacement"' "$postinstall"
grep -Fq 'python3 "$rename_exchange" "$migration_replacement" "$migration_file"' "$postinstall"
grep -Fq "trap 'abort_migration \$?' 0" "$postinstall"

echo "postinstall config migration tests passed"
