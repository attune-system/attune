#!/usr/bin/env bash
set -euo pipefail

root=$(mktemp -d "${TMPDIR:-/tmp}/attune-runtime-log-safety.XXXXXX")
trap 'rm -rf "$root"' EXIT

cat >"$root/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ $1 == container && $2 == inspect ]]; then
    if [[ $* == *--format* ]]; then
        printf '%s\n' foreign-owner
    fi
    exit 0
fi
printf '%s\n' "$*" >>"$MUTATION_LOG"
exit 99
EOF
cat >"$root/kubectl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$KUBECTL_LOG"
exit 99
EOF
chmod +x "$root/docker" "$root/kubectl"

export MUTATION_LOG="$root/docker-mutations"
export KUBECTL_LOG="$root/kubectl-calls"
touch "$MUTATION_LOG" "$KUBECTL_LOG"

if PATH="$root:$PATH" \
    RUNTIME_LOG_HARNESS_OWNER=expected-owner \
    RUNTIME_LOG_S3_CONTAINER=collision \
    RUNTIME_LOG_S3_NETWORK=collision \
    RUNTIME_LOG_S3_IMAGE=s3 \
    RUNTIME_LOG_AWS_CLI_IMAGE=aws-cli \
    RUNTIME_LOG_S3_PORT=59000 \
    RUNTIME_LOG_S3_ACCESS_KEY=user \
    RUNTIME_LOG_S3_SECRET_KEY=password \
    RUNTIME_LOG_S3_BUCKET=bucket \
    RUNTIME_LOG_S3_PREFIX=runtime-log-tests/expected-owner \
    scripts/runtime-log-s3.sh up >/dev/null 2>&1; then
    printf '%s\n' 'S3 harness accepted a foreign container collision' >&2
    exit 1
fi
[[ ! -s $MUTATION_LOG ]]

if PATH="$root:$PATH" \
    RUNTIME_LOG_HARNESS_OWNER=expected-owner \
    RUNTIME_LOG_S3_CONTAINER=owned \
    RUNTIME_LOG_S3_NETWORK=owned \
    RUNTIME_LOG_S3_IMAGE=s3 \
    RUNTIME_LOG_AWS_CLI_IMAGE=aws-cli \
    RUNTIME_LOG_S3_PORT=59000 \
    RUNTIME_LOG_S3_ACCESS_KEY=user \
    RUNTIME_LOG_S3_SECRET_KEY=password \
    RUNTIME_LOG_S3_BUCKET=bucket \
    RUNTIME_LOG_S3_PREFIX=runtime-log-tests \
    scripts/runtime-log-s3.sh clean-prefix >/dev/null 2>&1; then
    printf '%s\n' 'S3 harness accepted an unscoped cleanup prefix' >&2
    exit 1
fi
[[ ! -s $MUTATION_LOG ]]

if PATH="$root:$PATH" scripts/runtime-log-rwx-conformance.sh >/dev/null 2>&1; then
    printf '%s\n' 'RWX harness accepted missing destructive-operation confirmation' >&2
    exit 1
fi
[[ ! -s $KUBECTL_LOG ]]

printf '%s\n' 'runtime-log harness safety checks passed'
