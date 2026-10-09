#!/usr/bin/env bash
set -euo pipefail

mode="${1:-}"
container_id="${POSTGRES_CONTAINER_ID:?POSTGRES_CONTAINER_ID is required}"
database="${POSTGRES_DB:-attune_test}"
user="${POSTGRES_USER:-attune}"
run_id="${ATTUNE_TEST_RUN_ID:?ATTUNE_TEST_RUN_ID is required for ownership-scoped cleanup}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9-]{0,19}$ ]]; then
    echo "ERROR: ATTUNE_TEST_RUN_ID must be 1-20 lowercase ASCII letters/digits with optional non-leading '-'" >&2
    exit 2
fi
run_token="${run_id//-/_}"
schema_prefix="test_${run_token}_"
database_prefix="attune_db_${run_token}_"
template_prefix="attune_tpl_${run_token}_"

psql_admin() {
    docker exec -i "$container_id" \
        psql -X -U "$user" -d "$database" -v ON_ERROR_STOP=1 -qAt "$@"
}

count_named() {
    local catalog="$1"
    local column="$2"
    local prefix="$3"
    psql_admin -c "SELECT count(*) FROM ${catalog} WHERE left(${column}, ${#prefix}) = '${prefix}';"
}

count_schemas() { count_named pg_catalog.pg_namespace nspname "$schema_prefix"; }
count_databases() { count_named pg_catalog.pg_database datname "$database_prefix"; }
count_templates() { count_named pg_catalog.pg_database datname "$template_prefix"; }

report_targets() {
    echo "Run-owned test databases:"
    psql_admin -c \
        "SELECT '  ' || quote_ident(datname) FROM pg_catalog.pg_database WHERE left(datname, ${#database_prefix}) = '${database_prefix}' ORDER BY datname;"
    echo "Run-owned template databases:"
    psql_admin -c \
        "SELECT '  ' || quote_ident(datname) FROM pg_catalog.pg_database WHERE left(datname, ${#template_prefix}) = '${template_prefix}' ORDER BY datname;"
    echo "Legacy run-owned test schemas:"
    psql_admin -c \
        "SELECT '  ' || quote_ident(nspname) FROM pg_catalog.pg_namespace WHERE left(nspname, ${#schema_prefix}) = '${schema_prefix}' ORDER BY nspname;"
}

require_numeric_counts() {
    local value
    for value in "$@"; do
        [[ "$value" =~ ^[0-9]+$ ]] || {
            echo "ERROR: PostgreSQL returned a non-numeric resource count" >&2
            exit 2
        }
    done
}

drop_databases_with_prefix() {
    local prefix="$1"
    local templates="${2:-false}"
    while IFS= read -r database_name; do
        [[ -z "$database_name" ]] && continue
        if [[ "$templates" == true ]]; then
            psql_admin -c "ALTER DATABASE \"$database_name\" IS_TEMPLATE false;" </dev/null
        fi
        psql_admin -c "DROP DATABASE \"$database_name\" WITH (FORCE);" </dev/null
    done < <(psql_admin -c \
        "SELECT datname FROM pg_catalog.pg_database WHERE left(datname, ${#prefix}) = '${prefix}' ORDER BY datname;")
}

case "$mode" in
    baseline)
        schema_count="$(count_schemas)"
        database_count="$(count_databases)"
        template_count="$(count_templates)"
        require_numeric_counts "$schema_count" "$database_count" "$template_count"

        echo "Baseline resources: databases=$database_count templates=$template_count schemas=$schema_count"
        if (( schema_count > 0 || database_count > 0 || template_count > 0 )); then
            report_targets
            echo "ERROR: run-owned database resources already exist before tests." >&2
            exit 1
        fi

        if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
            {
                echo "schema_count=$schema_count"
                echo "database_count=$database_count"
                echo "template_count=$template_count"
            } >> "$GITHUB_OUTPUT"
        fi
        ;;
    cleanup)
        schema_count="$(count_schemas)"
        database_count="$(count_databases)"
        template_count="$(count_templates)"
        baseline_schemas="${BASELINE_SCHEMA_COUNT:-}"
        baseline_databases="${BASELINE_DATABASE_COUNT:-}"
        baseline_templates="${BASELINE_TEMPLATE_COUNT:-}"
        require_numeric_counts \
            "$schema_count" "$database_count" "$template_count" \
            "$baseline_schemas" "$baseline_databases" "$baseline_templates"

        leak_detected=false
        if (( schema_count != baseline_schemas || database_count != baseline_databases )); then
            leak_detected=true
        fi

        echo "Baseline resources: databases=$baseline_databases templates=$baseline_templates schemas=$baseline_schemas"
        echo "Post-test resources: databases=$database_count templates=$template_count schemas=$schema_count"
        if (( schema_count > 0 || database_count > 0 || template_count > 0 )); then
            report_targets
        fi

        # Per-test database clones are dropped first. Templates are intentional
        # run-level build artifacts and are removed only after every clone.
        drop_databases_with_prefix "$database_prefix"
        drop_databases_with_prefix "$template_prefix" true

        while IFS= read -r drop_statement; do
            [[ -z "$drop_statement" ]] && continue
            psql_admin -c "SET client_min_messages TO warning; $drop_statement" </dev/null
        done < <(psql_admin -c \
            "SELECT format('DROP SCHEMA %I CASCADE', nspname) FROM pg_catalog.pg_namespace WHERE left(nspname, ${#schema_prefix}) = '${schema_prefix}' ORDER BY nspname;")

        remaining_schemas="$(count_schemas)"
        remaining_databases="$(count_databases)"
        remaining_templates="$(count_templates)"
        require_numeric_counts "$remaining_schemas" "$remaining_databases" "$remaining_templates"
        echo "Post-cleanup resources: databases=$remaining_databases templates=$remaining_templates schemas=$remaining_schemas"
        if (( remaining_schemas != 0 || remaining_databases != 0 || remaining_templates != 0 )); then
            echo "ERROR: run-owned database objects remain after cleanup." >&2
            report_targets >&2
            exit 1
        fi
        if [[ "$leak_detected" == true ]]; then
            echo "ERROR: tests leaked per-test database resources before janitor recovery." >&2
            exit 1
        fi
        ;;
    *)
        echo "Usage: $0 {baseline|cleanup}" >&2
        exit 2
        ;;
esac
