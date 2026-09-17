#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
cd "$repo_root"

assert_absent() {
    local pattern=$1
    shift
    if grep -En "$pattern" "$@"; then
        printf 'credential output pattern found: %s\n' "$pattern" >&2
        exit 1
    fi
}

assert_absent '^[[:space:]]*(echo|printf).*Bearer \$(TOKEN|ACCESS_TOKEN)' \
    scripts/quick-test-happy-path.sh scripts/setup_timer_echo_rule.sh \
    scripts/setup-test-rules.sh
assert_absent 'Bearer \\\$(TOKEN|ACCESS_TOKEN)' \
    scripts/quick-test-happy-path.sh scripts/setup_timer_echo_rule.sh \
    scripts/setup-test-rules.sh
assert_absent '(Response: \$(LOGIN_RESPONSE|AUTH_RESPONSE)|echo "\$RESPONSE_BODY")' \
    scripts/quick-test-happy-path.sh scripts/setup_timer_echo_rule.sh \
    scripts/test-timer-echo-docker.sh web/scripts/test-cors.sh
assert_absent '^[[:space:]]*(echo|printf|print_info).*(Password:.*\$(password|ADMIN_PASSWORD|TEST_PASSWORD)|password.*\$(ADMIN_PASSWORD|TEST_PASSWORD))' \
    scripts/create_test_user.sh scripts/create-test-user.sh
assert_absent '(Database URL: \$DATABASE_URL|Target database: \$DATABASE_URL|DB:.*\$DATABASE_URL"$|URL: \$DB_URL)' \
    scripts/load-core-pack.sh scripts/cleanup-test-schemas.sh \
    scripts/verify-schema-cleanup.sh scripts/test-db-setup.sh \
    docker/rust-tests-entrypoint.sh
assert_absent 'echo.*postgres(ql)?://[^[:space:]]+@' \
    scripts/setup-e2e-db.sh scripts/start-e2e-services.sh scripts/test-db-setup.sh
assert_absent '(token\[:[0-9]+\]|secret_value\[:[0-9]+\]|= \{secret_value\}|original: \{secret_value\})' \
    tests/*.py tests/e2e/tier*/*.py
assert_absent 'expected.*\{secret_value\}' \
    tests/*.py tests/e2e/tier*/*.py
assert_absent 'cat web/\.env' web/scripts/*.sh
assert_absent 'println!\("  (Password|Hash|New hash): \{\}"' test_password.rs

url_output_scripts=(
    scripts/attune-agent-wrapper.sh
    docker/inject-env.sh
    docker/e2e-entrypoint.sh
    scripts/generate-python-client.sh
    scripts/setup_timer_echo_rule.sh
    scripts/setup-test-rules.sh
    scripts/test-timer-echo-docker.sh
    scripts/delete-legacy-gitea-linux-packages.sh
    scripts/test-webhook-event-processing.sh
)
assert_absent '^[[:space:]]*(echo|printf)[[:space:]].*\$\{?(API_URL|WS_URL|AGENT_URL|ATTUNE_API_URL|GITEA_BASE_URL|OPENAPI_SPEC_URL)(\}|[^A-Z0-9_])' \
    "${url_output_scripts[@]}"
for script in "${url_output_scripts[@]}"; do
    grep -Fq 'sanitize_url_origin()' "$script"
    grep -Fq "printf '%s\\n' '<url configured>'" "$script"
    grep -Fq 'authority=${authority%%/*}' "$script"
    grep -Fq "*[!0-9]*) printf '%s\\n' '<url configured>'" "$script"
    grep -Fq "printf '%s://%s\\n'" "$script"
done

assert_absent 'echo[[:space:]]+"?\$(EXEC_RESPONSE|RESPONSE_DATA|PACK_RESPONSE|TRIGGER_RESPONSE|ACTION_RESPONSE|CREATE_RESPONSE|CREATE_TRIGGER_RESPONSE|CREATE_RULE_RESPONSE|RULE1|RULE2|RULE3)"?[[:space:]]*$|echo[[:space:]]+"?\$(RULE1|RULE2|RULE3)"?[[:space:]]*\|[[:space:]]*jq[[:space:]]+\.|printf.*\$install_response' \
    scripts/test-completion-fix.sh scripts/test-webhook-event-processing.sh \
    scripts/reinstall-standard-packs.sh scripts/setup_timer_echo_rule.sh \
    scripts/setup-test-rules.sh scripts/test-timer-echo-docker.sh
assert_absent 'cat[[:space:]]+"?\$(response_file|TEMP_SPEC)|result: \.result\.stdout' \
    scripts/delete-legacy-gitea-linux-packages.sh scripts/generate-python-client.sh \
    scripts/test-timer-echo-docker.sh
assert_absent '(Error details: \{error_data\}|Response text: \{e\.response\.text|Health check passed: \{data\})' \
    tests/quick_test.py
assert_absent '(println!|eprintln!|panic!).*(Body:|body_text|response\.text)|"\{(body|browse_body)\}"|unexpected deletion error: \{error\}' \
    crates/api/tests/health_and_auth_tests.rs crates/api/tests/pack_registry_tests.rs
assert_absent 'assert_eq!.*(Authorization|access_token|refresh_token)' \
    crates/api/tests/health_and_auth_tests.rs crates/api/tests/pack_registry_tests.rs

grep -Fq 'chmod 600 .env' docker/quickstart.sh
grep -Fq 'chmod 600 /build/config.test.yaml' docker/rust-tests-entrypoint.sh

database_url_scripts=(
    scripts/cleanup-test-schemas.sh
    scripts/load-core-pack.sh
    scripts/verify-schema-cleanup.sh
    docker/rust-tests-entrypoint.sh
)
for script in "${database_url_scripts[@]}"; do
    grep -Fq 'local at_signs=${1//[^@]/}' "$script"
    grep -Fq "printf '%s\\n' '<database-url configured>'" "$script"
    grep -Fq "s#[?#].*\$##" "$script"
    if grep -Fq 's#(://)[^/@]*@#' "$script"; then
        printf 'unsafe database URL sanitizer found: %s\n' "$script" >&2
        exit 1
    fi
done

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/attune-credential-output.XXXXXX")
trap 'rm -rf -- "$test_dir"' EXIT
cat >"$test_dir/psql" <<'EOF'
#!/usr/bin/env bash
printf '0\n'
EOF
chmod +x "$test_dir/psql"

database_output=$(
    PATH="$test_dir:$PATH" \
    ATTUNE_TEST_RUN_ID='credential-test' \
    DATABASE_URL='postgresql://test-user:live-password@database.example:6543/attune?sslmode=require#fragment-secret' \
        scripts/cleanup-test-schemas.sh
)
[[ $database_output == *'Target database: postgresql://database.example:6543/attune'* ]]
[[ $database_output != *'test-user'* ]]
[[ $database_output != *'live-password'* ]]
[[ $database_output != *'sslmode'* ]]
[[ $database_output != *'fragment-secret'* ]]

malformed_database_output=$(
    PATH="$test_dir:$PATH" \
    ATTUNE_TEST_RUN_ID='credential-test' \
    DATABASE_URL='postgresql://test-user:live-password@password-tail@database.example:6543/attune?query-secret#fragment-secret' \
        scripts/cleanup-test-schemas.sh
)
[[ $malformed_database_output == *'Target database: <database-url configured>'* ]]
[[ $malformed_database_output != *'test-user'* ]]
[[ $malformed_database_output != *'live-password'* ]]
[[ $malformed_database_output != *'password-tail'* ]]
[[ $malformed_database_output != *'query-secret'* ]]
[[ $malformed_database_output != *'fragment-secret'* ]]

url_output=$(
    GITEA_BASE_URL='https://registry-user:live-password@packages.example:8443/private/path?token=query-secret#fragment-secret' \
        scripts/delete-legacy-gitea-linux-packages.sh --dry-run --version sha-test
)
[[ $url_output == *'Gitea origin: https://packages.example:8443'* ]]
[[ $url_output != *'registry-user'* ]]
[[ $url_output != *'live-password'* ]]
[[ $url_output != *'private/path'* ]]
[[ $url_output != *'query-secret'* ]]
[[ $url_output != *'fragment-secret'* ]]

malformed_url_output=$(
    GITEA_BASE_URL='https://name:port-secret/private/path' \
        scripts/delete-legacy-gitea-linux-packages.sh --dry-run --version sha-test
)
[[ $malformed_url_output == *'Gitea origin: <url configured>'* ]]
[[ $malformed_url_output != *'port-secret'* ]]
[[ $malformed_url_output != *'private/path'* ]]

cat >"$test_dir/curl" <<'EOF'
#!/usr/bin/env bash
output_file=""
request_url=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -o) output_file=$2; shift 2 ;;
        *) request_url=$1; shift ;;
    esac
done
printf '%s\n' "$request_url" >"$CURL_URL_FILE"
printf '#!/bin/sh\nexit 0\n' >"$output_file"
EOF
chmod +x "$test_dir/curl"

agent_url='https://agent-user:agent-password@agent.example:9443/private/agent?token=download-secret'
agent_output=$(
    PATH="$test_dir:$PATH" \
    CURL_URL_FILE="$test_dir/curl-url" \
    ATTUNE_AGENT_DIR="$test_dir/agent" \
    ATTUNE_AGENT_URL="$agent_url" \
    ATTUNE_AGENT_TOKEN='agent-bootstrap-secret' \
    ATTUNE_AGENT_ARCH=x86_64 \
        scripts/attune-agent-wrapper.sh
)
[[ $agent_output == *'URL origin: https://agent.example:9443'* ]]
[[ $agent_output != *'agent-user'* ]]
[[ $agent_output != *'agent-password'* ]]
[[ $agent_output != *'private/agent'* ]]
[[ $agent_output != *'download-secret'* ]]
[[ $(<"$test_dir/curl-url") == "${agent_url}?arch=x86_64" ]]

grep -Fq "Authorization: Bearer <token>" scripts/quick-test-happy-path.sh
grep -Fq "Authorization: Bearer <token>" scripts/setup_timer_echo_rule.sh
grep -Fq "Authorization: Bearer <token>" scripts/setup-test-rules.sh

printf 'credential output safety checks passed\n'
