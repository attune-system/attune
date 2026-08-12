# Core Pack Tests

These tests exercise core action behavior, failure handling, entry-point permissions,
and action metadata contracts.

## Running Tests

From the repository root:

```bash
./packs/core/tests/run_tests.sh
python3 -m unittest packs/core/tests/test_actions.py -v
pytest packs/core/tests/test_actions.py -v
attune pack check packs/core
attune pack test packs/core --detailed
```

`test_pack_installation_actions.sh` is a focused validation runner for the pack API
wrapper actions:

```bash
./packs/core/tests/test_pack_installation_actions.sh
```

## Parameter Delivery

Tests invoke actions through the delivery contract declared in action YAML. Action
parameters are not environment variables.

Shell actions declare `parameter_delivery: stdin` and
`parameter_format: dotenv`:

```bash
printf "message='hello'\n" | ./packs/core/actions/echo.sh
printf "seconds='0'\nmessage='no delay'\n" | ./packs/core/actions/sleep.sh
```

Nested objects use dotted dotenv names, matching the worker formatter:

```bash
printf "url='http://127.0.0.1:8080'\nquery_params.page='1'\n" \
  | ./packs/core/actions/http_request.sh
```

Python actions that declare `parameter_format: json` receive one flat JSON object on
stdin:

```bash
printf '%s\n' '{"key_ref":"example-key"}' \
  | python3 ./packs/core/actions/generate_ssh_key_pair.py
```

Variables such as `ATTUNE_API_URL` and `ATTUNE_API_TOKEN` are execution context,
not action parameter delivery.

## Coverage

- `echo.sh`: provided, omitted, empty, and special-character messages
- `noop.sh`: messages, exit-code boundaries, and invalid exit codes
- `sleep.sh`: default/zero durations, timing, messages, and invalid values
- `http_request.sh`: local GET/query, body, methods, 404, timeout, and missing URL
- API wrapper actions: required-input failures and structured JSON output
- All action YAML: stdin delivery, supported formats, output schemas, and entry points

HTTP behavior tests use a local server and do not require external network access or
the Python `requests` package. PyYAML is required for metadata assertions; pytest is
optional because the configured pack runner uses `unittest`.

## Adding Tests

Use `CorePackTestCase.run_action` with a flat parameter dictionary. It formats
dotenv or JSON stdin in the same shape as the worker:

```python
stdout, stderr, code = self.run_action(
    "my_action.sh",
    {"param": "value"},
)
```

For shell smoke coverage, pipe the declared format to the action rather than setting
parameter-like environment variables. Keep success assertions exact and verify both
non-zero status and structured error content for failure paths.
