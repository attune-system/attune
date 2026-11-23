#!/usr/bin/env bash
set -euo pipefail
image="${1:-postgres:18-alpine}"
cd "$(dirname "${BASH_SOURCE[0]}")/.."
probe=$(docker run --rm -d -e POSTGRES_USER=attune -e POSTGRES_PASSWORD=attune -p 127.0.0.1::5432 "$image")
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
trap 'docker stop "$probe" >/dev/null; rm -f "$fixture_log"' EXIT
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
