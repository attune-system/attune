#!/usr/bin/env bash
set -euo pipefail

mode="${1:-}"
container_id="${POSTGRES_CONTAINER_ID:?POSTGRES_CONTAINER_ID is required}"
database="${POSTGRES_DB:-attune_test}"
user="${POSTGRES_USER:-attune}"

psql_admin() {
    docker exec -i "$container_id" \
        psql -X -U "$user" -d "$database" -v ON_ERROR_STOP=1 -qAt "$@"
}

schema_count_sql="SELECT count(*) FROM pg_catalog.pg_namespace WHERE left(nspname, 5) = 'test_';"
job_count_sql="SELECT count(*) FROM timescaledb_information.jobs WHERE left(hypertable_schema, 5) = 'test_';"

count_schemas() {
    psql_admin -c "$schema_count_sql"
}

count_jobs() {
    psql_admin -c "$job_count_sql"
}

report_targets() {
    echo "Temporary test schemas:"
    psql_admin -c \
        "SELECT '  ' || quote_ident(nspname) FROM pg_catalog.pg_namespace WHERE left(nspname, 5) = 'test_' ORDER BY nspname;"
    echo "Timescale jobs targeting temporary test schemas:"
    psql_admin -c \
        "SELECT format('  job_id=%s procedure=%I.%I hypertable=%I.%I', job_id, proc_schema, proc_name, hypertable_schema, hypertable_name) FROM timescaledb_information.jobs WHERE left(hypertable_schema, 5) = 'test_' ORDER BY job_id;"
}

case "$mode" in
    baseline)
        schema_count="$(count_schemas)"
        job_count="$(count_jobs)"

        echo "Baseline temporary test schemas: $schema_count"
        echo "Baseline Timescale jobs targeting temporary test schemas: $job_count"
        if (( schema_count > 0 || job_count > 0 )); then
            report_targets
        fi

        if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
            {
                echo "schema_count=$schema_count"
                echo "job_count=$job_count"
            } >> "$GITHUB_OUTPUT"
        fi
        ;;
    cleanup)
        schema_count="$(count_schemas)"
        job_count="$(count_jobs)"
        baseline_schemas="${BASELINE_SCHEMA_COUNT:-unavailable}"
        baseline_jobs="${BASELINE_JOB_COUNT:-unavailable}"

        echo "Baseline counts: schemas=$baseline_schemas jobs=$baseline_jobs"
        echo "Post-test counts: schemas=$schema_count jobs=$job_count"
        if (( schema_count > 0 || job_count > 0 )); then
            echo "Detected temporary database leftovers:"
            report_targets
            psql_admin <<'SQL'
BEGIN;

DO $cleanup_jobs$
DECLARE
    target_job_id integer;
BEGIN
    FOR target_job_id IN
        SELECT job_id
        FROM timescaledb_information.jobs
        WHERE left(hypertable_schema, 5) = 'test_'
        ORDER BY job_id
    LOOP
        PERFORM delete_job(target_job_id);
    END LOOP;
END
$cleanup_jobs$;

DO $cleanup_schemas$
DECLARE
    target_schema text;
BEGIN
    FOR target_schema IN
        SELECT nspname
        FROM pg_catalog.pg_namespace
        WHERE left(nspname, 5) = 'test_'
        ORDER BY nspname
    LOOP
        EXECUTE format('DROP SCHEMA %I CASCADE', target_schema);
    END LOOP;
END
$cleanup_schemas$;

COMMIT;
SQL
        else
            echo "No temporary database leftovers detected."
        fi

        remaining_schemas="$(count_schemas)"
        remaining_jobs="$(count_jobs)"
        echo "Post-cleanup counts: schemas=$remaining_schemas jobs=$remaining_jobs"
        if (( remaining_schemas != 0 || remaining_jobs != 0 )); then
            echo "ERROR: temporary database objects remain after cleanup." >&2
            report_targets >&2
            exit 1
        fi
        ;;
    *)
        echo "Usage: $0 {baseline|cleanup}" >&2
        exit 2
        ;;
esac
