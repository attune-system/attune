# Attune core pack

Core 1.0.7 ships four actions: `core.echo`, `core.sleep`, `core.noop`, and `core.http_request`.
They are standalone static native binaries. No Python, shell, curl, or other
interpreter/tool is required to execute them. Parameters remain a flat JSON
object on stdin; the text actions keep text output and HTTP emits structured JSON.

- `echo`: writes `message` verbatim, including trailing newlines; default empty.
- `sleep`: waits `seconds` (integer 0–3600; default 1), optionally prints `message`.
- `noop`: optionally prints `message`, returns `exit_code` (integer 0–255; default 0).
- `http_request`: sends HTTP(S) requests and emits the prior structured response
  fields (`status_code`, `headers`, `body`, `json`, `elapsed_ms`, `url`, `success`).
  Methods, headers, raw/JSON body, query parameters, basic/bearer auth, request
  timeout, certificate verification and redirect controls remain available.
  `json_body` takes precedence over `body`; 301/302 POST and 303 non-HEAD
  redirects become GET without a body, while 307/308 preserve method/body.
  Default TLS verification uses bundled public roots. `verify_ssl: false` is an
  explicit opt-out, not the default. Response buffering is limited to16MiB;
  non-UTF-8 response bodies are decoded lossily. Header line breaks and URL userinfo
  are rejected; configure credentials through the secret-marked auth parameters.
  Standard Authorization/Cookie/Proxy-Authorization headers are removed across
  origins (scheme/host/port). Other headers and replayed bodies remain application
  data; disable redirects when those data must not reach a redirected destination.
  This is an intentionally general HTTP client, not a destination allowlist or
  worker network sandbox. Only HTTP(S) is permitted initially and on every hop.
  Non-2xx responses retain status/body and exit1; configuration/transport failures
  return status0 and generic errors without embedding input credential values.

The agent-command, SSH-key, pack-installation actions and installation
workflow have been removed. There is no implicit replacement or optional utility
pack in this release. Put application-specific code in explicitly installed packs
and use the normal CLI/API for pack administration.

Timer sensors, trigger definitions, runtimes, dashboards, and permission definitions
are retained. The timer sensor launcher is separate from these four actions.
No action receives API authority by default; explicit permission selection and
recorded/delegated authority rules remain unchanged.

## Packaging

Each native action's `entry_point` is `bin/<name>`, a directory containing regular
executable files named `linux-amd64` and/or `linux-arm64`. The worker selects its
own platform in Rust and executes that file directly. Missing/unsupported targets,
unknown directory entries, symlinks, and path traversal fail before process spawn.
Ordinary native file entrypoints retain their existing behavior.

Official init-packs images include **both** static variants and publish identical
core bytes on both architectures. Local Compose builds contain exactly the host
variant and assemble a private candidate without copying old action metadata.
Local binary initialization stages under a separate `pack_binaries` volume, mounted
read-only by the publisher and absent from workers, outside the active core projection; bootstrap copies only fresh binary files into its
private candidate.
Do not publish a local host-only archive as the official cross-architecture release.
Binaries are built using `docker/Dockerfile.pack-binaries` / cargo-zigbuild, as with
the timer sensor. Source checkout alone is not an executable pack: build artifacts
must be assembled before publication; there is no interpreter/source fallback.

Core 1.0.7 has a new immutable version/digest binding. Normal bootstrap explicitly
requests the existing **pack-wide** `absent_metadata_policy=remove`. Complete retained
nonaction definitions remain in the candidate; omitted actions and the installer's
owned workflow definition retire. Omitted actions cannot
be resolved for new work. Historical records and already-admitted pinned executions
retain their original immutable release. Do not reset databases, delete retained
release trees, or republish changed bytes under an existing stable version.

See [testing](TESTING.md) and [deployment setup](SETUP.md).

The restored native HTTP implementation changes this candidate's bytes. Version
1.0.7 remains provisional until release coordination verifies that it has not
already been published immutably; discovery of an existing digest requires a
coordinated pack/chart version bump, never replacement of that artifact.
