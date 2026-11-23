#!/usr/bin/env bash
set -euo pipefail
image="${1:-postgres:18-alpine}"
cd "$(dirname "${BASH_SOURCE[0]}")/.."
probe=$(docker run --rm -d -e POSTGRES_USER=attune -e POSTGRES_PASSWORD=attune \
    -e 'POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256 --auth-local=trust' -p 127.0.0.1::5432 "$image" -c authentication_timeout=2s)
trap 'docker stop "$probe" >/dev/null' EXIT
db() { docker exec -i "$probe" "$@"; }
ready=false
for _ in {1..60}; do
    if db pg_isready -h 127.0.0.1 -U attune </dev/null >/dev/null 2>&1; then ready=true; break; fi
    sleep 0.25
done
test "$ready" = true
address=$(docker port "$probe" 5432/tcp)
fixture_log=$(mktemp /tmp/attune-db-role-migration-XXXXXX)
fixture_client_pids=()
tls_dir=
cleanup() {
    docker stop "$probe" >/dev/null
    for pid in "${fixture_client_pids[@]}"; do
        wait "$pid" || true
    done
    rm -f "$fixture_log"
    if [[ -n "$tls_dir" ]]; then rm -r "$tls_dir"; fi
}
trap cleanup EXIT
make db-test-setup TEST_DB_ADMIN_URL="postgresql://attune:attune@$address/postgres" TEST_DB_URL="postgresql://attune:attune@$address/attune_test" > "$fixture_log" 2>&1 || { tail -30 "$fixture_log"; exit 1; }
printf '%s migrations passed\n' "$image"
admin() { db psql -X -q -v ON_ERROR_STOP=1 -U attune -d attune_test "$@"; }
admin <<'SQL'
CREATE ROLE operations_owner NOLOGIN;
CREATE ROLE action_ops NOLOGIN;
CREATE ROLE sensor_ops NOLOGIN;
CREATE ROLE action_a LOGIN;
CREATE ROLE action_b LOGIN;
CREATE ROLE sensor_a LOGIN;
CREATE ROLE unbound LOGIN;
GRANT action_ops TO action_a, action_b, unbound WITH INHERIT TRUE, SET FALSE;
GRANT sensor_ops TO sensor_a WITH INHERIT TRUE, SET FALSE;
INSERT INTO manager_session (role, pool, name, instance, enrollment_digest, expires_at)
VALUES
 ('action','build','worker-a','00000000-0000-0000-0000-000000000001',repeat('a',64),clock_timestamp()+interval '5 minutes'),
 ('action','build','worker-b','00000000-0000-0000-0000-000000000002',repeat('a',64),clock_timestamp()+interval '5 minutes'),
 ('sensor','observe','sensor-a','00000000-0000-0000-0000-000000000003',repeat('b',64),clock_timestamp()+interval '5 minutes'),
 ('action','build','worker-unbound','00000000-0000-0000-0000-000000000005',repeat('a',64),clock_timestamp()+interval '5 minutes');
SQL
admin -v app_schema=attune -v operation_owner=operations_owner -v action_group=action_ops -v sensor_group=sensor_ops -f - < deployment/worker-db-roles/provision.sql >/dev/null
bind() {
    admin -v app_schema=attune -v login_name="$1" -v manager_role="$2" -v manager_pool="$3" \
        -v manager_slot="$4" -v manager_instance="$5" -f - < deployment/worker-db-roles/bind.sql >/dev/null
}
a=00000000-0000-0000-0000-000000000001
b=00000000-0000-0000-0000-000000000002
s=00000000-0000-0000-0000-000000000003
bind action_a action build worker-a "$a"
bind action_b action build worker-b "$b"
bind sensor_a sensor observe sensor-a "$s"
bind action_a action build worker-a "$a"
admin </dev/null -c "GRANT SELECT (name) ON manager_session TO unbound"
if bind unbound action build worker-unbound 00000000-0000-0000-0000-000000000005 2>/dev/null; then
    printf 'FAIL manager login with a base table column grant was bound\n' >&2
    exit 1
fi
admin </dev/null -c "REVOKE SELECT (name) ON manager_session FROM unbound"
printf 'PASS login with a base table column grant cannot bind\n'
query() { db psql -X -At -v ON_ERROR_STOP=1 -U "$1" -d attune_test -c "$2" </dev/null; }
check() {
    local got
    got=$(query "$1" "$2")
    if [[ "$got" != "$3" ]]; then printf 'FAIL %s (got %s; expected %s)\n' "$4" "$got" "$3"; return 1; fi
    printf 'PASS %s\n' "$4"
}
check action_a "SELECT count(*) FROM manager_db_action_session('$a') WHERE closed_at IS NULL" 1 'action can inspect own boot'
check sensor_a "SELECT count(*) FROM manager_db_sensor_session('$s') WHERE closed_at IS NULL" 1 'sensor can inspect own boot'
check action_a "SELECT count(*) FROM manager_db_action_session('$b', true)" 0 'same-role peer withdrawal denied'
check unbound "SELECT count(*) FROM manager_db_action_session('$a', true)" 0 'unbound member denied'
check action_a "SELECT has_function_privilege('manager_db_sensor_session(uuid,boolean)','EXECUTE')" f 'action has no sensor entry point'
check sensor_a "SELECT has_function_privilege('manager_db_action_session(uuid,boolean)','EXECUTE')" f 'sensor has no action entry point'
check action_a "SELECT has_table_privilege('manager_session','SELECT,UPDATE') OR has_table_privilege('manager_db_principal','SELECT,UPDATE') OR has_table_privilege('execution','SELECT,UPDATE') OR has_schema_privilege('attune','CREATE') OR pg_has_role('operations_owner','SET')" f 'runtime login has no underlying or owner privileges'
check action_a "SELECT count(*) FROM manager_db_action_session('$a', true) WHERE closed_at IS NOT NULL" 1 'exact action withdrawal succeeds'
check action_a "SELECT count(*) FROM manager_db_action_session('$a', true) WHERE closed_at IS NOT NULL" 1 'same withdrawal retry is safe'
admin <<'SQL'
INSERT INTO manager_session (role, pool, name, instance, enrollment_digest, expires_at)
VALUES ('action', 'build', 'worker-a', '00000000-0000-0000-0000-000000000004', repeat('a',64), clock_timestamp() + interval '5 minutes');
SQL
replacement=00000000-0000-0000-0000-000000000004
if bind action_a action build worker-a "$replacement" 2>/dev/null; then
    printf 'FAIL retired login was rebound to a replacement instance\n' >&2
    exit 1
fi
check action_a "SELECT count(*) FROM manager_db_action_session('$replacement', true)" 0 'retired login cannot withdraw replacement'
check sensor_a "SELECT count(*) FROM manager_db_sensor_session('$s', true) WHERE closed_at IS NOT NULL" 1 'exact sensor withdrawal succeeds'
admin </dev/null -c "UPDATE manager_db_principal SET revoked_at = clock_timestamp() WHERE login_name = 'action_b'"
check action_b "SELECT count(*) FROM manager_db_action_session('$b', true)" 0 'revoked principal cannot withdraw'
check attune "SELECT count(*) FROM manager_session WHERE instance = '$b' AND closed_at IS NULL" 1 'peer remains open'
admin </dev/null -c "UPDATE manager_session SET expires_at = clock_timestamp() WHERE instance = '$a'"
check action_a "SELECT count(*) FROM manager_db_action_session('$a', true)" 0 'expired session no longer authorizes access'
query attune 'SHOW server_version'

admin <<'SQL'
CREATE ROLE retirement_owner NOLOGIN CREATEROLE;
CREATE ROLE retirement_caller LOGIN;
INSERT INTO manager_session (role, pool, name, instance, enrollment_digest, expires_at)
VALUES
 ('action','build','managed-live','00000000-0000-0000-0000-000000000006',
        repeat('a',64),clock_timestamp()+interval '5 minutes'),
 ('sensor','observe','managed-sensor','00000000-0000-0000-0000-000000000007',
        repeat('b',64),clock_timestamp()+interval '5 minutes');
SQL
admin -v app_schema=attune -v retirement_owner=retirement_owner -v retirement_caller=retirement_caller \
    -v login_prefix=fixture_mgr -v action_group=action_ops -v sensor_group=sensor_ops \
    -v database=attune_test -v api_role=attune -f - \
    < deployment/worker-db-roles/provision-retirement.sql >/dev/null
check retirement_caller "SELECT has_table_privilege('manager_db_credential','SELECT,UPDATE')
    OR pg_has_role('retirement_owner','SET') OR pg_has_role('pg_signal_backend','USAGE')
    OR (SELECT rolcreaterole FROM pg_roles WHERE rolname = current_user)" f 'reconciler has only guarded SQL authority'

tls_dir=$(mktemp -d /tmp/attune-manager-role-tls-XXXXXX)
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=127.0.0.1' \
    -addext 'subjectAltName=IP:127.0.0.1' -keyout "$tls_dir/key.pem" -out "$tls_dir/cert.pem" \
    >/dev/null 2>&1
docker exec -i --user postgres "$probe" sh -ec 'umask 077; cat > /tmp/attune-role-tls-key.pem' < "$tls_dir/key.pem"
docker exec -i --user postgres "$probe" sh -ec 'umask 077; cat > /tmp/attune-role-tls-cert.pem' < "$tls_dir/cert.pem"
admin <<'SQL'
ALTER SYSTEM SET ssl_cert_file = '/tmp/attune-role-tls-cert.pem';
ALTER SYSTEM SET ssl_key_file = '/tmp/attune-role-tls-key.pem';
ALTER SYSTEM SET ssl = 'on';
SELECT pg_reload_conf();
SQL
for _ in {1..80}; do
    if [[ "$(query attune 'SHOW ssl')" == on ]]; then break; fi
    sleep 0.1
done
check attune 'SHOW ssl' on 'disposable database serves TLS'

verifier() {
    python3 - "$1" <<'PY'
import base64, hashlib, hmac, sys
salt = bytes(range(16))
key = hashlib.pbkdf2_hmac("sha256", sys.argv[1].encode(), salt, 4096)
stored = hashlib.sha256(hmac.digest(key, b"Client Key", "sha256")).digest()
server = hmac.digest(key, b"Server Key", "sha256")
encode = lambda value: base64.b64encode(value).decode()
print(f"SCRAM-SHA-256$4096:{encode(salt)}${encode(stored)}:{encode(server)}")
PY
}
first=fixture-password-first
second=fixture-password-second
admin -v verifier="$(verifier "$first")" <<'SQL'
INSERT INTO manager_db_credential
    (manager_session, pod_uid, request_id, requested_generation, proof_expires_at, pending_verifier)
SELECT id, instance, instance, 1, clock_timestamp() + interval '2 minutes', :'verifier'
FROM manager_session WHERE name IN ('managed-live', 'managed-sensor');
SQL
check retirement_caller 'SELECT install_requested_manager_db_logins(32)' 2 'restricted caller issues both manager roles'
check retirement_caller 'SELECT install_requested_manager_db_logins(32)' 0 'install retry has no side effect'
managed=$(query attune "SELECT login_name FROM manager_db_credential c JOIN manager_session s ON c.manager_session=s.id WHERE s.name='managed-live'")
sensor=$(query attune "SELECT login_name FROM manager_db_credential c JOIN manager_session s ON c.manager_session=s.id WHERE s.name='managed-sensor'")
authenticate() {
    db env PGPASSWORD="$1" PGSSLMODE=verify-full PGSSLROOTCERT=/tmp/attune-role-tls-cert.pem \
        psql -X -h 127.0.0.1 -At -v ON_ERROR_STOP=1 -U "$2" -d attune_test -c "$3" </dev/null
}
test "$(authenticate "$first" "$managed" "SELECT count(*) FROM manager_db_action_session('00000000-0000-0000-0000-000000000006')")" = 1
test "$(authenticate "$first" "$sensor" "SELECT count(*) FROM manager_db_sensor_session('00000000-0000-0000-0000-000000000007')")" = 1
printf 'PASS issued SCRAM credentials authenticate scoped action and sensor logins over verified TLS\n'
check "$managed" "SELECT has_function_privilege('install_requested_manager_db_logins(integer)','EXECUTE')
    OR has_function_privilege('disable_retired_manager_db_logins(integer)','EXECUTE')
    OR has_table_privilege('manager_db_credential','SELECT,UPDATE')" f 'manager lacks provisioning authority'
admin -v verifier="$(verifier "$second")" <<'SQL'
UPDATE manager_db_credential c SET requested_generation = 2, pending_verifier = :'verifier',
    request_id = gen_random_uuid(), proof_expires_at=clock_timestamp()+interval '2 minutes'
FROM manager_session s WHERE c.manager_session=s.id AND s.name='managed-live';
SQL
check retirement_caller 'SELECT install_requested_manager_db_logins(32)' 1 'rotation installs one new generation'
if authenticate "$first" "$managed" 'SELECT 1' >/dev/null 2>&1; then
    printf 'FAIL rotated password still authenticates\n' >&2
    exit 1
fi
test "$(authenticate "$second" "$managed" 'SELECT 1')" = 1
printf 'PASS only the new password authenticates\n'

wait_for_activity() {
    local expected=$1
    for _ in {1..80}; do
        if [[ "$(query attune "SELECT count(*) FROM pg_stat_activity
            WHERE application_name = 'manager-retirement-fixture' AND state = 'active'")" == "$expected" ]]; then
            return
        fi
        sleep 0.1
    done
    printf 'FAIL waiting for %s manager fixture sessions\n' "$expected" >&2
    return 1
}
db env PGAPPNAME=manager-retirement-fixture psql -X -U "$managed" -d attune_test \
    -c "BEGIN; SELECT * FROM manager_db_action_session('00000000-0000-0000-0000-000000000006'); SELECT pg_sleep(90)" \
    </dev/null >/dev/null 2>&1 &
fixture_client_pids+=("$!")
for login in action_a "$sensor"; do
    db env PGAPPNAME=manager-retirement-fixture psql -X -U "$login" -d attune_test \
        -c 'SELECT pg_sleep(90)' </dev/null >/dev/null 2>&1 &
    fixture_client_pids+=("$!")
done
wait_for_activity 3
# Expire the login ledger while the action connection holds binding and session
# locks, as an ordinary caller transaction does.
admin -v login="$managed" <<'SQL'
UPDATE manager_db_credential SET credential_expires_at = clock_timestamp() WHERE login_name = :'login';
SQL
check retirement_caller 'SELECT disable_retired_manager_db_logins(32)' 1 'retirement fences login while binding lock is held'
if authenticate "$second" "$managed" 'SELECT 1' >/dev/null 2>&1; then
    printf 'FAIL retired role still permits connections\n' >&2
    exit 1
fi
check retirement_caller 'SELECT drain_retired_manager_db_connections(32)' 0 'signal delivery awaits drain confirmation'
wait_for_activity 2
for _ in {1..80}; do
    if [[ "$(query retirement_caller 'SELECT drain_retired_manager_db_connections(32)')" == 1 ]]; then
        break
    fi
    sleep 0.1
done
check attune "SELECT count(*) FROM manager_db_credential WHERE login_name = '$managed'
    AND retired_at IS NOT NULL AND drained_at IS NOT NULL" 1 'reconciler confirms old connection is gone'
check retirement_caller 'SELECT disable_retired_manager_db_logins(32) + drain_retired_manager_db_connections(32)' 0 'retirement retry is idempotent'
check attune "SELECT count(*) FROM pg_roles WHERE rolname IN ('action_a', '$sensor') AND rolcanlogin" 2 'unmanaged and live managed logins remain enabled'
check retirement_caller 'SELECT count(*) FROM claim_manager_db_pod_checks(16)' 1 'only the live manager is due for a Kubernetes check'
check retirement_caller 'SELECT count(*) FROM claim_manager_db_pod_checks(16)' 0 'a claimed Kubernetes check waits for its next scheduled pass'
sensor_id=$(query attune "SELECT manager_session FROM manager_db_credential WHERE login_name = '$sensor'")
query retirement_caller "SELECT invalidate_manager_db_pod($sensor_id, gen_random_uuid())" >/dev/null
check retirement_caller 'SELECT disable_retired_manager_db_logins(32)' 0 'a stale Pod identity cannot retire the live SQL login'
query retirement_caller "SELECT invalidate_manager_db_pod($sensor_id, '00000000-0000-0000-0000-000000000007')" >/dev/null
check retirement_caller 'SELECT disable_retired_manager_db_logins(32)' 1 'confirmed Pod invalidation retires the exact login'
