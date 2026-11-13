# Core action entrypoints

Core actions receive one flat JSON object on stdin (`parameter_delivery: stdin`,
`parameter_format: json`). Values are never parsed as physical `key=value` lines
or expanded into environment variables. There is no dotenv fallback.

The shell entrypoints require Python 3.11 or newer (standard library only).
`http_request` also requires curl. These dependencies must exist in the selected worker image;
the actions declare `required_worker_runtimes` so a shell-only worker is not
eligible. The bundled Compose shell and Node workers install Python during
bootstrap; custom workers must advertise a verified matching Python runtime.
The pack API actions use the same shell-wrapper/Python-helper pattern.

```sh
printf '%s' '{"message":"First line\nSecond line"}' | ./echo.sh
printf '%s' '{"seconds":0,"message":"No delay"}' | ./sleep.sh
printf '%s' '{"exit_code":0,"message":"Done"}' | ./noop.sh
printf '%s' '{"url":"http://127.0.0.1:8080/health","headers":{"Accept":"application/json"}}' | ./http_request.sh
```

`http_request` constructs curl argv directly, uses `--url` explicitly, disables
curl config files and URL glob expansion, and restricts initial and redirect
protocols to HTTP/HTTPS. Bodies use a private temporary file and `--data-binary`
to preserve bytes, including trailing newlines. `json_body` remains an object;
`headers` and `query_params` remain objects, with query keys and values encoded.
HTTP destination selection remains the caller's responsibility: these controls
are not a network destination allowlist. Curl still interprets HTTP header syntax.

`run_agent_command.command` intentionally contains shell code. Other fields,
including multiline `state_dir`, remain literal values. The action requires the
worker-provided execution API URL and token; it does not mint authority, add
permission refs, or fall back to another credential. `require_mcp_binary` defaults
to true. Execution authority and the reserved scoped `standard` permission set
retain their platform contracts.

From the core pack root, run the real-entrypoint regressions and owned local
HTTP smoke tests with:

```sh
./tests/run_tests.sh
```

The fixtures use dummy context credentials, a local fake curl, and an ephemeral
loopback HTTP server. They do not require a live Attune service or public HTTP API.
