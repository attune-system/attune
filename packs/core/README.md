# Attune core pack

Core 1.0.7 ships exactly three actions: `core.echo`, `core.sleep`, and `core.noop`.
They are standalone static native binaries. No Python, shell, curl, or other
interpreter/tool is required to execute them. Parameters remain a flat JSON
object on stdin; output remains text.

- `echo`: writes `message` verbatim, including trailing newlines; default empty.
- `sleep`: waits `seconds` (integer 0–3600; default 1), optionally prints `message`.
- `noop`: optionally prints `message`, returns `exit_code` (integer 0–255; default 0).

The HTTP, agent-command, SSH-key, pack-installation actions and installation
workflow have been removed. There is no implicit replacement or optional utility
pack in this release. Put application-specific code in explicitly installed packs
and use the normal CLI/API for pack administration.

Timer sensors, trigger definitions, runtimes, dashboards, and permission definitions
are retained. The timer sensor launcher is separate from these three actions.
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
nonaction definitions remain in the candidate; omitted actions retire and cannot
be resolved for new work. Historical records and already-admitted pinned executions
retain their original immutable release. Do not reset databases, delete retained
release trees, or republish changed bytes under an existing stable version.

See [testing](TESTING.md) and [deployment setup](SETUP.md).
