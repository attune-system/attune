# Core validation

The standalone Rust crate owns input/output and process regression tests:

```sh
CARGO_BUILD_JOBS=2 cargo test --manifest-path crates/core-actions/Cargo.toml --locked -- --test-threads=4
python3 scripts/test_core_bootstrap_composition.py
```

No runtime test runner is declared by core: the currently supported candidate
runners require an interpreter. Native tests run at build/publication time instead.

The worker's explicit fixture lane requires real static AMD64 and ARM64 binaries
under `ATTUNE_CORE_ACTION_BIN_ROOT/{x86_64,aarch64}-unknown-linux-musl/release/`:

```sh
ATTUNE_CORE_ACTION_BIN_ROOT="$PWD/crates/core-actions/target" ATTUNE_TEST_RUN_ID=core-native \
  CARGO_BUILD_JOBS=2 cargo run -p attune-worker --example core_native_archive
```

This lane publishes the actual core tree, checks deterministic archives, safely
materializes archive bytes, deletes authoring/source releases, and invokes the
real NativeRuntime with an unusable PATH and no API token. For HTTP, the actual
archived production binary must reject an owned loopback destination with status0,
exit1 and no TCP connection; this is a denial test, not public-network success.
Its nonblocking listener and any accepted socket close before assertions, with
no background server to time out or leak on runtime failure. The explicit lane
fails if its prebuilt fixtures are missing; it never silently skips or passes.
Component lifecycle DB regressions verify omission retirement independently.

HTTP's production process suite proves non-public destination denials before any
request reaches owned loopback listeners, with no tools or inherited credentials:

```sh
ATTUNE_CORE_HTTP_BINARY=/absolute/path/to/attune-core-http_request python3 scripts/test_native_http_action.py
```

The test harness needs Python and OpenSSL solely to create an ephemeral self-signed
TLS mock; neither is a worker/action prerequisite. It runs against debug and real
static AMD64 binaries in CI; ARM64 must also build and pass strict ELF packaging.

The standalone Cargo suite also runs the existing HTTP/TLS positive, redirect,
credential, response-size and deadline cases through a `cfg(test)`-only loopback
transport harness. The bypass is absent from production and static binaries; it
is not an environment/configuration option of the action. Policy tests check
public DNS answer preservation, mixed/private DNS rejection and normalized IP
forms without public/cloud probes. Local mock transport success does not claim
an Internet/public-TLS integration run. Production binaries reject loopback even
when given an invented caller policy parameter.
