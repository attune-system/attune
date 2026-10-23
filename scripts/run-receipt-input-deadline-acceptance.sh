#!/usr/bin/env bash
# Explicit external-service acceptance. Never included implicitly in default CI.
# Invoke with bash. Own both services, isolated vhosts, twelve serial test runs,
# fresh run IDs and awaited teardown. Preserve logs under /tmp/opencode.
set -euo pipefail
for tool in docker python3 psql sqlx cargo make; do
    command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool" >&2; exit 2; }
done
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$root"
prefix=scheduler::native_failure_tests::receipt_deadline
cases=(
    original_finite_input_actual_completion_commits_and_publishes_before_expiry_control
    original_finite_input_expires_at_actual_completion_commit_and_publishes_nothing
    publication_wait::original_finite_later_input_two_publications_before_expiry_control
    publication_wait::original_finite_later_input_expires_during_first_application_handoff_wait
    publication_wait::publisher_recovery::original_finite_input_real_publisher_recovery_before_expiry_control
    publication_wait::publisher_recovery::original_finite_input_denies_new_attempt_after_real_publisher_recovery_wait
)
focus_target="${ATTUNE_RECEIPT_ACCEPTANCE_FOCUS_TARGET:-}"
focus_case="${ATTUNE_RECEIPT_ACCEPTANCE_FOCUS_CASE:-}"
expected_executions=12
if [[ -n "$focus_target" || -n "$focus_case" ]]; then
    if [[ "$focus_target" != lib && "$focus_target" != bin ]]; then
        echo "Focused receipt execution requires target lib or bin and one exact inventory case" >&2
        exit 2
    fi
    known_case=false
    for candidate in "${cases[@]}"; do
        if [[ "$candidate" == "$focus_case" ]]; then known_case=true; fi
    done
    if [[ "$known_case" != true ]]; then
        echo "Focused receipt execution requires one exact inventory case" >&2
        exit 2
    fi
    expected_executions=1
fi
if [[ "${ATTUNE_RECEIPT_ACCEPTANCE_RUN_ID+x}" == x ]]; then
    run="$ATTUNE_RECEIPT_ACCEPTANCE_RUN_ID"
else
    run="$(python3 -c 'import secrets; print("r06"+secrets.token_hex(7))')"
fi
if [[ ! "$run" =~ ^[a-z0-9][a-z0-9-]{0,19}$ ]]; then
    echo "Receipt acceptance run ID must be 1-20 lowercase letters/digits with non-leading hyphens" >&2
    exit 2
fi
password="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
if [[ "${ATTUNE_RECEIPT_ACCEPTANCE_OUT+x}" == x ]]; then
    logs="$ATTUNE_RECEIPT_ACCEPTANCE_OUT"
    if [[ -z "$logs" ]] || ! (umask 077; mkdir -m 700 -- "$logs"); then
        echo "Receipt acceptance output must be an absent fresh directory with an existing owned parent" >&2
        exit 2
    fi
    logs="$(cd "$logs" && pwd -P)"
else
    logs="$(mktemp -d /tmp/opencode/receipt-deadline.XXXXXX)"
fi
python3 - "$logs" "$run" "$expected_executions" "$focus_target" "$focus_case" <<'PY'
import json, os, pathlib, sys
os.umask(0o077)
out = pathlib.Path(sys.argv[1])
run = sys.argv[2]
record = {
    "owner": run, "out": str(out.resolve()),
    "containers": [f"attune-receipt-pg-{run}", f"attune-receipt-mq-{run}"],
    "labels": {"attune.validation.owner": run, "attune.receipt-deadline": run},
    "cargo": {"toolchain": "1.99.0", "locked": True, "offline": True,
              "allFeatures": True, "buildJobs": 1, "rustMinStack": 67108864,
              "testThreads": 4, "serialExecutions": int(sys.argv[3])},
    "scope": "full required inventory" if int(sys.argv[3]) == 12 else "focused diagnostic only",
    "focusTarget": sys.argv[4], "focusCase": sys.argv[5],
    "fixtureTiming": {"singleChildConsentSeconds": 20,
                      "pairRecoveryConsentSeconds": 120,
                      "pairControlMarginSeconds": 10,
                      "recoveryTransportSeconds": 150,
                      "ownedBrokerHeartbeatSeconds": 180},
}
with (out / "run-controls.json").open("x") as stream:
    json.dump(record, stream, indent=2)
    stream.write("\n")
PY
echo "Receipt acceptance owner: $run"
echo "Receipt acceptance output: $logs"
containers=()
cleanup() {
    status=$?
    trap - EXIT INT TERM
    failed=0
    for container in "${containers[@]}"; do
        # IDs are recorded only after our successful docker create. No ambient
        # compose project, developer container, volume or broker is cleaned up.
        docker rm --force --volumes "$container" >/dev/null || failed=1
    done
    echo "Receipt input acceptance logs: $logs"
    if (( failed )); then
        echo "Owned service teardown failed; acceptance is not clean" >&2
        exit 1
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
pg="$(docker create --name "attune-receipt-pg-$run" \
    --label "attune.receipt-deadline=$run" \
    --label "attune.validation.owner=$run" \
    --publish 127.0.0.1::5432 \
    --env POSTGRES_USER=attune --env "POSTGRES_PASSWORD=$password" \
    postgres:18-alpine)"
containers+=("$pg")
mq="$(docker create --name "attune-receipt-mq-$run" \
    --label "attune.receipt-deadline=$run" \
    --label "attune.validation.owner=$run" \
    --publish 127.0.0.1::5672 \
    --env RABBITMQ_DEFAULT_USER=attune --env "RABBITMQ_DEFAULT_PASS=$password" \
    --env "RABBITMQ_SERVER_ADDITIONAL_ERL_ARGS=-rabbit heartbeat 180" \
    rabbitmq:4.3.6-management-alpine)"
containers+=("$mq")
echo "Receipt acceptance owned PostgreSQL ID: $pg"
echo "Receipt acceptance owned RabbitMQ ID: $mq"
docker start "$pg" "$mq" >/dev/null
deadline=$((SECONDS + 120))
until docker exec "$pg" pg_isready -U attune -d postgres >/dev/null 2>&1; do
    (( SECONDS < deadline )) || { echo "Owned PostgreSQL was not ready" >&2; exit 1; }
    sleep 0.25
done
until docker exec "$mq" rabbitmq-diagnostics -q check_running >/dev/null 2>&1 &&
    docker exec "$mq" rabbitmq-diagnostics -q check_port_connectivity >/dev/null 2>&1; do
    (( SECONDS < deadline )) || { echo "Owned RabbitMQ was not ready" >&2; exit 1; }
    sleep 0.25
done
# This disposable broker's long heartbeat is for the synthetic recovery gate.
# A client-only request could negotiate down to the server's default 60s.
if [[ "$(docker exec "$mq" rabbitmqctl -q eval 'application:get_env(rabbit, heartbeat).')" != "{ok,180}" ]]; then
    echo "Owned receipt broker did not apply its test-only 180s heartbeat" >&2
    exit 1
fi
for vhost in receipt-input-control receipt-input-expiry receipt-publication-control receipt-publication-expiry receipt-recovery-control receipt-recovery-expiry; do
    docker exec "$mq" rabbitmqctl add_vhost "$vhost" >/dev/null
    docker exec "$mq" rabbitmqctl set_permissions -p "$vhost" attune '.*' '.*' '.*' >/dev/null
done
pg_address="$(docker port "$pg" 5432/tcp)"
mq_address="$(docker port "$mq" 5672/tcp)"
export TEST_DB_ADMIN_URL="postgresql://attune:$password@127.0.0.1:${pg_address##*:}/postgres"
export TEST_DB_URL="postgresql://attune:$password@127.0.0.1:${pg_address##*:}/attune_test"
export ATTUNE__DATABASE__URL="$TEST_DB_URL"
export DATABASE_URL="$TEST_DB_URL"
# Overwrite, never fall back to, an inherited developer broker URL.
export ATTUNE_TEST_CACHE_BROKER_URL="amqp://attune:$password@127.0.0.1:${mq_address##*:}"
export CARGO_TERM_COLOR=never
# Never archive the generated database password from Make/SQLx diagnostics.
# pipefail preserves setup failure; the one-shot redactor has no service role.
make --silent db-test-setup 2>&1 | ATTUNE_RECEIPT_REDACT_TOKEN="$password" python3 -c '
import os, sys
token = os.environ["ATTUNE_RECEIPT_REDACT_TOKEN"]
for line in sys.stdin:
    sys.stdout.write(line.replace(token, "<redacted>"))
    sys.stdout.flush()
' >"$logs/database-setup.log"
# Explicit inventory: 6 exact identities in EACH executor target. No parallel
# targets, and no empty selection/ignored default result may count as acceptance.
executed=0
for target in lib bin; do
    if [[ -n "$focus_target" && "$target" != "$focus_target" ]]; then continue; fi
    selection=(--lib)
    if [[ "$target" == bin ]]; then selection=(--bin attune-executor); fi
    for case_name in "${cases[@]}"; do
        if [[ -n "$focus_case" && "$case_name" != "$focus_case" ]]; then continue; fi
        identity="$prefix::$case_name"
        invocation="$(python3 -c 'import secrets; print("r06"+secrets.token_hex(7))')"
        log="$logs/$target-${case_name//::/-}.log"
        echo "Explicit acceptance: $target $identity"
        ATTUNE_TEST_RUN_ID="$invocation" CARGO_BUILD_JOBS=1 RUST_MIN_STACK=67108864 \
            cargo +1.99.0 test --locked --offline --all-features -p attune-executor "${selection[@]}" "$identity" -- \
            --exact --ignored --nocapture --test-threads=4 2>&1 | \
            ATTUNE_RECEIPT_REDACT_TOKEN="$password" python3 -u -c '
import os, sys
token = os.environ["ATTUNE_RECEIPT_REDACT_TOKEN"]
for line in sys.stdin:
    sys.stdout.write(line.replace(token, "<redacted>"))
    sys.stdout.flush()
' | tee "$log"
        python3 - "$log" "$identity" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text()
identity = sys.argv[2]
if not re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", text):
    raise SystemExit("acceptance requires exactly one executed passing test, not a default ignored/empty selection")
if not re.search(r"test " + re.escape(identity) + r" \.\.\. ok", text):
    raise SystemExit("exact required acceptance identity did not execute successfully")
PY
        executed=$((executed + 1))
    done
done
if [[ "$executed" != "$expected_executions" ]]; then
    echo "Receipt execution count does not match its explicit selection" >&2
    exit 1
fi
if [[ "$expected_executions" == 12 ]]; then
    echo "All twelve explicit receipt-input identities executed successfully; service teardown still required."
else
    echo "One focused receipt execution passed; full twelve-execution acceptance remains outstanding."
fi
