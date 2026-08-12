#!/bin/sh
# Focused shell smoke tests using the actions' declared stdin/dotenv contract.

set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ACTIONS_DIR=$(dirname "$SCRIPT_DIR")/actions
TOTAL=0
PASSED=0
FAILED=0
FAILED_NAMES=""

run_test() {
    name=$1
    shift
    TOTAL=$((TOTAL + 1))
    if "$@"; then
        PASSED=$((PASSED + 1))
        printf 'PASS %s\n' "$name"
    else
        FAILED=$((FAILED + 1))
        FAILED_NAMES="${FAILED_NAMES}\n  ${name}"
        printf 'FAIL %s\n' "$name"
    fi
}

assert_output() {
    expected=$1
    input=$2
    script=$3
    output=$(printf '%b' "$input" | /bin/sh "$ACTIONS_DIR/$script" 2>/dev/null)
    code=$?
    [ "$code" -eq 0 ] && [ "$output" = "$expected" ]
}

assert_exit() {
    expected=$1
    input=$2
    script=$3
    printf '%b' "$input" | /bin/sh "$ACTIONS_DIR/$script" >/dev/null 2>&1
    code=$?
    [ "$code" -eq "$expected" ]
}

assert_http_missing_url() {
    output=$(printf '\n' | /bin/sh "$ACTIONS_DIR/http_request.sh" 2>/dev/null)
    code=$?
    [ "$code" -ne 0 ] && printf '%s' "$output" | python3 -c \
        'import json,sys; value=json.load(sys.stdin); assert value["success"] is False and value["error"] == "url parameter is required"'
}

assert_action_contracts() {
    PACK_DIR=$(dirname "$ACTIONS_DIR") python3 - <<'PY'
import os
from pathlib import Path
import yaml

actions = Path(os.environ["PACK_DIR"]) / "actions"
for path in actions.glob("*.yaml"):
    action = yaml.safe_load(path.read_text())
    assert action.get("parameter_delivery") == "stdin", path
    assert action.get("parameter_format") in {"dotenv", "json"}, path
    assert "output_schema" not in action, path
    entry_point = action.get("entry_point")
    if entry_point and action.get("runner_type") != "native":
        assert (actions / entry_point).is_file(), (path, entry_point)
PY
}

run_test "echo receives stdin message" assert_output "Hello, Attune!" "message='Hello, Attune!'\n" echo.sh
run_test "echo defaults to empty output" assert_output "" "" echo.sh
run_test "echo accepts an empty message" assert_output "" "message=''\n" echo.sh
run_test "noop default succeeds" assert_output "No operation completed successfully" "" noop.sh
run_test "noop logs its message" assert_output "[NOOP] Test message
No operation completed successfully" "message='Test message'\n" noop.sh
run_test "noop accepts exit code 0" assert_exit 0 "exit_code='0'\n" noop.sh
run_test "noop returns requested exit code" assert_exit 5 "exit_code='5'\n" noop.sh
run_test "noop accepts maximum exit code" assert_exit 255 "exit_code='255'\n" noop.sh
run_test "noop rejects negative exit code" assert_exit 1 "exit_code='-1'\n" noop.sh
run_test "noop rejects oversized exit code" assert_exit 1 "exit_code='256'\n" noop.sh
run_test "noop rejects non-numeric exit code" assert_exit 1 "exit_code='abc'\n" noop.sh
run_test "sleep accepts zero seconds" assert_output "Slept for 0 seconds" "seconds='0'\n" sleep.sh
run_test "sleep prints its message" assert_output "Sleeping now...
Slept for 0 seconds" "message='Sleeping now...'\nseconds='0'\n" sleep.sh
run_test "sleep rejects negative seconds" assert_exit 1 "seconds='-1'\n" sleep.sh
run_test "sleep rejects oversized seconds" assert_exit 1 "seconds='3601'\n" sleep.sh
run_test "sleep rejects non-numeric seconds" assert_exit 1 "seconds='abc'\n" sleep.sh
run_test "http request reports missing URL as JSON" assert_http_missing_url
run_test "action YAML declares current parameter delivery" assert_action_contracts

printf 'Total Tests: %s\n' "$TOTAL"
printf 'Passed: %s\n' "$PASSED"
printf 'Failed: %s\n' "$FAILED"
printf 'Skipped: 0\n'

if [ "$FAILED" -ne 0 ]; then
    printf 'Failed tests:%b\n' "$FAILED_NAMES"
    exit 1
fi
