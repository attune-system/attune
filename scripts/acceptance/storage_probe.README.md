# Owned storage lifecycle probe

`storage_probe.py` implements E's `storage_probe_command` stdin/stdout protocol.
It dispatches actual `core.run_agent_command` with the repo-owned Python producer,
observes a real running worker and encrypted file publication, keeps that exact
execution alive across upgrade, then cancels and observes native process exit.
It performs exact physical cleanup fault/retry and checks real quarantine denial.
It does not start services, build, install packs, create database fixtures or
implement an unknown-writer settlement controller.

## Binding

E can bind either profile without changing the driver protocol:

```json
"storage_probe_command": ["{python}", "{source_root}/scripts/acceptance/storage_probe.py", "--binding", "/owned/run/storage-probe.private.json", "--execute"]
```

Copy `storage_probe.binding.example.json` to a mode-0600 file owned by the driver
UID and fill its actual run/project/backend fields. Set `allow_native_faults` to
true only for the isolated labelled fixture. Use Python 3.11 or newer on the host
and inspection/action containers. The action host must have the artifact volume
mounted and access to its normal execution-scoped API environment.

SharedFile uses the same binding with `backend=SharedFile`, a real filesystem
blob root and the appropriate action-host/inspection services. ObjectSegments
requires a versioned RustFS service and the AWS CLI executable on the host. A
separate mode-0600 provider credential JSON file contains `project`, `bucket`,
`endpoint`, `region`, `access_key_id`, `secret_access_key`. Its bucket must equal
the driver's `owned_bucket`. Its endpoint must equal the binding and the real
owned provider container's loopback published port. Set `object_prefix` to the
actual provider prefix, if any. The probe issues real `get-bucket-versioning`,
bounded `list-object-versions` and `list-multipart-uploads`, checks full version
IDs, and fails on truncation. No provider credentials are placed on argv or sent
to the action.

The filesystem observation compares its actual inode, microsecond mtime and size
against the shipped object_store LocalFileSystem's recorded ETag encoding. It
also records the device and SHA256. A future incompatible provider encoding must
fail this fixture rather than fall back to path existence or HEAD absence.

The storage profile's ordinary invoker must explicitly allow `actions:execute`
for `core.run_agent_command` and own execution read/cancel. E's non-storage
core.echo/sleep/noop invoker is not sufficient for this fixture. This is a
powerful shell action, so use a disposable fixture identity. The producer uses
only `standard` runtime authority. It never receives
the human installer, maintenance operator, or provider token. No widening of
input-only consent or replacement of a receipt with runtime approval is involved.
The installer, invoker and maintenance operator are required to be three distinct
identities. The maintenance integration credential needs the exact run-owned
artifact read/delete grants and the existing retention maintenance management
requirements. Do not reuse the installer's token or a process token for cleanup.

### Authentication and private credential formats

Host authentication uses the registered root routes `/auth/token-login`,
`/auth/login` and `/auth/me`. Protected operations keep their `/api/v1/...` paths.
The host base may be the owned loopback origin or that origin with exactly
`/api/v1` appended. It is normalized to compose each registered route once;
`/api/v1/auth/*` aliases are rejected rather than invented. The worker producer's
internal API base is normalized separately, and its artifact/file calls remain
under `/api/v1`.

Installer and ordinary invoker files remain E's raw `attune_it_` integration
tokens. They use only `token_login` through `/auth/token-login`. Password JSON
and supplied human access tokens are rejected by that method. The maintenance
credential instead uses the exact JSON shape in
`storage_probe.maintenance-credential.example.json`: kind `password`, role
`maintenance`, run_id, owned_project, identity_id, login and password. E must
publicly provision that distinct fresh local account with a password. Its login
is `acceptance.<run_id with hyphens replaced by underscores>.maintenance`.
The file must be mode 0600, run-owned and in the same private directory as the
other two credential outputs. No original human or host-production credential
file is read as a maintenance fallback.

`password_login` sends only login/password to `/auth/login`; it does not request
or rewrite token scope. `/auth/me` confirms the provisioned role logins and three
distinct real identity IDs, including the maintenance ID in the private metadata.
The actual maintenance token must pass the audit capabilities endpoint before
dispatch. The server's `Access`/`scope=None` operator requirement is unchanged.
An integration-scoped access token that fails it stops the probe; no installer
retry or fallback token is used. Credentials are never passed on argv or printed.

Use exactly the driver's owner label and Compose project on all inspected
containers and named volumes. The binding lists every possible action writer
service. A producer argv marker must actually be observed on one before begin
can complete; all listed services must be available for native inspection. The
probe never removes a volume, runs Compose down, recursively deletes a host path,
or pauses an unlabelled service. Python performs no database queries. It invokes
the matching precompiled `storage_probe_inspector` over bounded stdin JSON. All
database access belongs to `StorageProbeInspectionRepository`; the adapter is a
local controlled-operator executable, not an unauthenticated HTTP dump endpoint.

### Repository-backed inspector prerequisite

The command source is `crates/common/examples/storage_probe_inspector.rs`, using
`crates/common/src/repositories/storage_probe_inspection.rs`. Central must compile
that example from the integrated source and record its measured SHA256 in
`inspector_sha256`. `inspector_binary` must be an absolute regular executable,
owned by the run UID or root and not writable by another user. This is a build
artifact prerequisite, not an arbitrary external fixture implementation. No
build is performed by the probe.

Create `inspector_context_file` from
`storage_probe.inspector-context.example.json`, mode 0600, owned by the driver UID.
It contains the actual loopback published database URL and application schema,
the exact run/project, the provisioned ordinary invoker's identity ID, an exact
allowlist of real old-writer fixture version IDs, and a fresh random identity key
of at least 32 bytes. Keep this key stable through all three phases. Use a local
observer DB credential with SELECT access to the repository's explicit columns;
no schema/data write, provisioning or secret decryption access is required. The
driver must bind the URL to its owned deployment, never a shared database.
Credentials and the identity key never appear on argv or in responses. The
context path is passed only in `ATTUNE_STORAGE_PROBE_INSPECTOR_CONTEXT` with other
inherited environment variables removed.

The inspector checks the exact execution's ordinary invoker, known action and
run-specific command marker before each observation. Its fixed operations are
`pins`, `version`, `backend`, `protected`, `ledger`, `released` and `legacy`.
There is no caller SQL, wildcard/table selection, query callback or arbitrary
projection option. One repeatable-read read-only transaction uses the configured
search_path, a five-second statement limit and bounded explicit column lists.
The adapter has a 15-second overall inspection bound and never prints raw DB
errors. Schema prefixes are not hardcoded.

The repository returns an authenticated exact-file identity receipt only after
observing that run's Ready version and provider/staging tuple. Later protected,
ledger and release requests must present it. This keeps inspection scoped after
metadata deletion without allowing Python to substitute another provider key or
version ID. The receipt authenticates a prior repository observation only. It
grants no API authority and proves no remote writer/request settlement. No config,
policy body, ciphertext, plaintext, original approval body or provider error text
is returned. Public API positive download and quarantine denial checks remain
mandatory; an inspection result is never their authorization fallback.

## Actual phases and evidence

- `begin` logs in as the ordinary invoker, dispatches the action, and writes its
  ID immediately to the driver's private state. It checks the recorded execution
  release ID, digest, policy revision, admission and authority digest through the
  typed repository-backed inspector. The first three must match E's fresh core
  pins. It waits for a
  Ready file, an authenticated file body, no unsettled PUT, the actual log backend,
  an observed native producer process and physical staging identity. The private
  state retains exact version/key/provider/staging identities and measured inode,
  device, size and digest. Provider inventory is measured before any fault.
- `upgraded` requires the old execution still running and compares its complete
  immutable authority/release tuple and exact file to the original observations.
  It also requires E's active upgraded policy to differ. Configuration equality
  is not used as evidence of execution pinning.
- `finish` checks rollback pins, cancels with the invoker and independently waits
  for native marker disappearance. For RustFS it pauses only the labelled owned
  provider container. For filesystem storage it moves the single observed object
  to a same-directory owned hold path and creates a different replacement
  generation at its old path. It then deletes the exact artifact version through
  the maintenance API and requires a new failed deletion attempt in that exact
  provider-qualified ledger row, while custody remains held. It restores the
  original object or unpauses the same container in `finally`, then observes the
  supervisor retry remove that exact ledger row, owner, pins and operations.
  Finally it observes actual staging removal and exact provider-version absence.
  It never calls an age, HEAD miss or cancellation remote settlement proof.

The failure is a known completed request's cleanup retry, not proof of recovery
from an unknown in-flight upload. The public producer's successful PUT plus its
authenticated persisted body and zero unsettled PUT operations establish the
known request boundary. No cached plaintext file is treated as settled by a
provider listing. `checks` are emitted only after all measured operations finish.
State is mode 0600 and records intermediate failures. Do not replay `finish`
after `cleanup_started`: inspect/restore the exact recorded native identities
before resuming manually. SIGKILL can interrupt fault restoration; the private
state retains its exact fault container or filesystem identity for recovery.

## Concrete remaining public API limitations

Public execution responses omit execution `pack_release`, `pack_release_digest`
and the admitted `policy_revision`/authority digest. The public evidence endpoint
omits object/staging identities, and successful physical release deletes the owner
rather than retaining a public tombstone. Public APIs expose no per-exact-object
cleanup attempt/error counter. Those are why this command uses the narrow native
repository inspection operations for these specific facts. They are not reasons
to bypass the repository layer with SQL in the Python driver.

Quarantine needs one **real old-writer fixture** supplied as `legacy_version_id`.
The probe checks its `meta.f_probe_run`, retained ownership and an actual
`file_evidence_put.protocol=legacy_unknown`, requests an audit, explicitly retains
manual quarantine, requires download HTTP 403 and verifies its unknown operation
and object custody remain. The current public API cannot create a legacy_unknown
operation or arbitrarily quarantine an atomic current PUT. Supplying that old
fixture is a live prerequisite, not permission to INSERT a fake operation or
claim unknown settlement. Absence hard-fails with the exact missing operation.
That version ID must also be in the private inspector context's exact allowlist.

Provider outage timeout must be shorter than `poll_seconds`; supervisor deletion
and staging-cleanup cycles must be enabled and fit that bound. An unsupported
fault, missing observer permission, provider truncation, filesystem precondition
failure, or unsatisfied legacy fixture fails the command. No default pass exists.

## Local tests only

```sh
python3 -m unittest discover -s scripts/acceptance -p 'test_storage_probe.py' -v
```

These tests use fake HTTP/native/provider transports. They do not validate real
Compose processes, worker dispatch, providers or production acceptance. Both live
backend positive paths still require central execution with actual immutable
images, credentials, labelled owned volumes and the old-writer fixture.

The action runtime for this fixture must execute as a local child inside the
listed labelled worker containers, with inspectable `/proc`. Detached host or
unlisted ephemeral-container runtimes are unsupported and fail the actual
producer-marker observation. API and provider proxy environment variables are
not used. The host API URL must be the driver's explicit loopback published port.
The core shell action's dotenv shim is handled by an apostrophe-free command;
tokens stay in the worker's normal execution environment.

The maintenance principal must first download the live protected payload and
match its exact bytes. A quarantine 403 must also contain the actual file evidence
readmission denial, not just an unrelated authentication/permission error.
