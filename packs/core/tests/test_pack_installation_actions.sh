#!/bin/sh
# Validation smoke tests for API-wrapper actions. Parameters are stdin/dotenv.

set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ACTIONS_DIR=$(dirname "$SCRIPT_DIR")/actions
TOTAL=0
PASSED=0
FAILED=0

assert_validation_failure() {
    script=$1
    field=$2
    output=$(printf '\n' | /bin/sh "$ACTIONS_DIR/$script" 2>/dev/null)
    code=$?
    [ "$code" -ne 0 ] || return 1
    printf '%s' "$output" | FIELD="$field" python3 -c \
        'import json,os,sys; value=json.load(sys.stdin); assert os.environ["FIELD"] in value'
}

run_test() {
    name=$1
    shift
    TOTAL=$((TOTAL + 1))
    if "$@"; then
        PASSED=$((PASSED + 1))
        printf 'PASS %s\n' "$name"
    else
        FAILED=$((FAILED + 1))
        printf 'FAIL %s\n' "$name"
    fi
}

run_test "get dependencies validates pack_paths" assert_validation_failure get_pack_dependencies.sh errors
run_test "download packs validates destination_dir" assert_validation_failure download_packs.sh failed_packs
run_test "build environments validates pack_paths" assert_validation_failure build_pack_envs.sh summary
run_test "register packs validates pack_paths" assert_validation_failure register_packs.sh failed_packs

printf 'Total Tests: %s\n' "$TOTAL"
printf 'Passed: %s\n' "$PASSED"
printf 'Failed: %s\n' "$FAILED"
printf 'Skipped: 0\n'
[ "$FAILED" -eq 0 ]
