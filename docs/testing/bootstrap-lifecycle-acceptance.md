# Owned bootstrap and storage lifecycle acceptance

`scripts/run-bootstrap-lifecycle-acceptance.py` is an integrator-run fixture
driver. Its default invocation prints a JSON plan without starting a process,
creating a directory, selecting a Docker context or contacting an API:

```sh
python3 scripts/run-bootstrap-lifecycle-acceptance.py --run-id ea-plan
```

The checked-in example deliberately contains unresolved artifact/fixture inputs.
It is not a runnable release configuration. An explicit `--execute` is required
for infrastructure, and unresolved prerequisites fail before Docker or builds:

```sh
python3 scripts/run-bootstrap-lifecycle-acceptance.py \
  --manifest /owned/acceptance/fixture.json \
  --source-root /owned/integrated-source \
  --run-id ea-unique123 --execute
```

No acceptance run, build, service launch or database job was performed when
authoring this source. Unit tests exercise planning and safety boundaries with
in-process mocks, not fake executables or a pretend running API.

## Central artifacts and prerequisites

Use `scripts/acceptance/bootstrap-lifecycle.example.json` as the manifest shape.
Every path in a runnable manifest should be absolute. Supply:

- Python 3.11 or later with PyYAML, a POSIX host, Docker Compose v2 and an explicit local Docker context using a
  Unix socket or named pipe. Remote contexts and inherited production contexts
  are not supported. No implicit image pull/build occurs.
- A retained integrated source snapshot containing the latest F preflight,
  migrations 06005 through 06008 and `file_evidence_retention_tests.rs`. Fill
  `migration_sha256` with measured SHA-256 for **every** path listed in the
  driver's `F_INPUTS`, including that Rust fixture source. The older standalone
  E task tree lacks some F prerequisites and must not masquerade as integrated.
- Actual immutable `image@sha256:...` references for every selected service and
  the init-packs image. API/consumer/init-agent image revision labels must match
  `source_revision`. Stock worker hosts must run that current injected agent.
  Central build provenance is required; image labels alone do not attest an
  uncommitted source snapshot. The backend probe must confirm real dispatch.
- The real prebuilt `file_evidence_retention_tests` binary, measured binary hash,
  and its build-time source/config snapshot. The runner checks its exact test
  inventory and requires each selected case to execute once, with zero ignored
  tests. No Cargo test invocation or hidden broad workspace build is added.
- A central prebuilt `bootstrap_acceptance_archive` example and its measured
  binary hash. Alternatively, the central build scheduler may explicitly permit
  the one helper build below. Service/native image builds remain centralized
  outside this driver.
- Two complete fixture profiles, **SharedFile and ObjectSegments**. Neither can
  be skipped. Each declares foundation, migration, initializer and writer
  services, current immutable images, existing source/config mounts, and actual
  foreground operator/backend/skew fixture commands. ObjectSegments additionally
  requires a run-owned versioned provider service and bucket initializer.

The helper build is opt-in and has exactly two Cargo jobs and an owned target
directory. Do not run it outside the centrally permissioned resource lane:

```sh
python3 scripts/run-bootstrap-lifecycle-acceptance.py \
  --manifest /owned/acceptance/fixture.json --source-root /owned/integrated-source \
  --run-id ea-build123 --execute --build-tools --permit-build
```

The archive helper copies no manufactured binaries. The driver copies actual
core content from a **stopped**, owned init-image container, compares source
files with the integrated tree and rejects unexpected extras. The Rust helper
requires real executable amd64 and arm64 ELF inputs and calls the same
`PackStorage::publish_release` implementation as API admission. It reports the
measured canonical archive digest, which Python independently hashes. A Python
upload tarball, OCI image digest or invented release pin is not substituted.
These local artifacts are never published to a release index by the driver.

## Credential and external fixture contract

The repo supplies `scripts/acceptance/provision_operator.py` for operator-owned
public-API provisioning and `scripts/acceptance/probe_old_image.py` for actual
old-API protocol rejection. The example resolves `{python}` and `{source_root}` at
runtime. Both profiles now invoke F's concrete repo `storage_probe.py` with a
generated private binding. F owns that implementation and its native inspector;
E neither copies nor rewrites them. No external PASS adapter or skipped profile
is shipped.

Commands receive one JSON document on stdin. It includes run/project ownership,
backend, loopback API URL, the private Compose-file path, source root and private
credential-output path. They must use only that run's resources, never daemonize
or escape their process group, and return one JSON result. No secret value goes
on argv or into the public plan. Do not echo credentials, payloads or HTTP bodies.

- `provision_command` is owned by the ordinary operator-provisioning fixture. It
  must supply an existing integration credential for this disposable deployment
  in the specified regular, run-owned mode-0600 file, and return
  `{"ordinary_operator_provisioning": true}`. It needs ordinary pack configure/
  install and test execution rights through the real catalog/auth APIs. The
  driver supplies no SQL application-user creation, role-assignment shortcut,
  signed human JWT, auth disable flag or temporary administrator backdoor.
  An `init-user` service is explicitly excluded. Initial operator provisioning
  remains an external prerequisite, not something a fresh database implies.
- `storage_probe_command` receives `phase=begin`, `upgraded`, then `finish`.
  Begin must dispatch a real pinned execution before upgrade, retain its state
  under the supplied private probe-state path, and return its positive execution
  ID and observed backend. During upgrade it must prove that same execution
  still uses its original release/policy and return `pinned_execution_retained`.
  Finish must await/join its writers and prove all named checks in `PROBE_CHECKS`.
  Those checks include actual backend selection, dispatch, retained execution,
  provider failure/retry with exact version identity, staging cleanup and
  quarantine denial. ObjectSegments must report observed bucket versioning
  `Enabled`. This is real transport/provider acceptance, not a JSON-shape test.
  F/C own those storage operations; the driver does not reimplement them.
  The probe must exchange the separate `invoker_credential_output` integration
  credential for dispatch. It must not borrow the installer token or ask for the
  human operator file. Additional F-owned artifact/cleanup authority, if needed,
  requires an explicitly reviewed separate fixture identity, not broader grants
  silently added to this installer/invoker pair.
- `skew_probe_command` must exercise an actual incompatible old image, not a
  fabricated HTTP server or executable. It must use the supplied owner/project
  labels for every extra container/network/volume, prove rejection before any
  publication, join its writers, and return `actual_old_image=true`,
  `rejected_before_publication=true`, `publication_requests=0`. An unavailable
  image or unrelated crash is failure, not skew acceptance. Its `image` must match
  the profile's immutable `old_image`. For `rejection_stage=api_protocol`, include
  observed HTTP status 200 and the actual incompatible `observed_protocol` value;
  for `client_contract`, include the actual incompatible `observed_client_version`.

The supplied skew implementation uses the API-protocol path. Each profile must
supply an actual incompatible `old_image` already present locally and an explicit
`old_expected_contract.bootstrap_admission_protocol`. Null means the old API is
expected to omit the protocol. Protocol 1 with a disabled gate does not count
as incompatible. The current API must first satisfy the full enabled admission
contract. The probe runs the actual old image's default API entrypoint on a new
loopback port in the verified owned network, using disposable API configuration
and separate empty owned pack/cache/body volumes, never current writable release
or artifact volumes. It does not receive installer, invoker or human credentials.
It uses the existing owned database only; this is a disposable deployment fixture,
not permission to point an old API at a normal development/production database.
Supply an API image whose normal startup does not run schema migrations or launch
other consumers. A crashing/schema-incompatible old image is a failed prerequisite,
not evidence of an admission-protocol rejection.

After observing HTTP 200 and the exact configured incompatible protocol, it calls
the shipped current publisher's real `wait_for_api`. A recorded HTTP adapter
allows only its readiness GET. Any attempted effect fails the probe. The publisher
must reject at that boundary after one GET. After stopping/waiting/removing the
old writer and exact inspected private volumes, the probe compares the current
API's persisted core release inventory, bootstrap release/policy pins and config
with its baseline. `publication_requests=0` describes the publisher request trace,
not a fabricated server-side global counter. The supported persisted APIs provide
the independent no-mutation check. A changed inventory is failure.

The public provisioning command creates separate passwordless installer/invoker
identities and a temporary password-authenticated issuer. It creates custom
permission sets through `/api/v1/permissions/sets`, assigns them through the
ordinary assignments API, verifies `/auth/me` effective grants, and mints expiring
integration tokens with the issuer's real `created_by`. Installer rights are only
core pack read/install/configure. Invoker rights are only core echo/sleep/noop
read/execute and own execution read/cancel. No admin role is assigned. It freezes
only its newly created issuer, verifies that issuer's access is denied, and checks
that both integration identities' old and newly exchanged access still work.
Independent frozen-owner and runtime approval tests remain separate.

Supply each profile's `operator_credential_file` as a run-user-owned mode-0600
JSON file with `owned_project` matching the planned project, or exact `api_url`,
and either `kind=password`, `login`, `password`, or `kind=access_token` with an
explicit human operator `access_token`. The latter must come from ordinary
human login/IdP, never a workload SDK/pack token. This file is read only by the
provisioner, not mounted in a container, copied into outputs, passed to storage
probes or included on argv. Output installer and invoker files contain separate
integration credentials, not that human credential. A human-provided access token
is an explicit trusted input, not something `/auth/me` proves to be human.

There is a real first-operator blocker on an empty local-auth database:
`routes/auth.rs::register` is disabled by default and, even when enabled, creates
an identity without permission assignments. `routes/permissions.rs::create_identity`
requires identities:create; creating/assigning grants requires permissions:manage.
Neither grants first-admin status. The API publisher expects an already existing
credential. This fixture therefore cannot manufacture the first administrator.
An operator must complete supported installation/IdP provisioning for the owned
deployment and supply its credential. Missing or denied onboarding fails without
SQL, seeded JWTs or a changed authorization guard. Also, identity admin routes use
targetless `authorize_permissions`, so the temporary issuer needs the single
global identities:update action to mint credentials. It is frozen immediately
after issuance. Adding ineffective target constraints would fake least privilege.

### Concrete F storage binding and third principal

Both profile commands are now:

```json
["{python}", "{source_root}/scripts/acceptance/storage_probe.py", "--binding", "{storage_binding}", "--execute"]
```

The driver replaces the binding placeholder with this profile's new mode-0600
`storage-probe.private.json`. It pins the actual F probe/producer source hashes
and the centrally built inspector binary hash. Stale Python SQL inspection and
unregistered versioned auth routes fail preflight rather than executing SQL or
silently trying a different transport. The inspector binding uses F's actual
flat `inspector_binary`, `inspector_sha256`, `inspector_context_file` fields.
The inspector context follows F's example exactly. Its invoker ID comes from
real public provisioning, its PG URL uses this project's engine-assigned loopback
port, its legacy allowlist contains only the supplied exact version, and its
fresh 32-byte random identity key stays stable across the three phases. It is
passed only through the inspector context path, never into a worker/action.

Each `storage_probe` settings block requires explicit consent to shell invocation,
maintenance management and owned native faults. The example leaves those false.
With consent, the fixture invoker has only `core.run_agent_command` read/execute
and own execution read/cancel. F requests `permission_set_refs=["standard"]`.
The ordinary echo/sleep/noop invoker remains unchanged outside this explicit
storage-fixture mode. No production default grant or pack runtime policy changes.

A third distinct local maintenance identity receives a fresh random password
through the protected CreateIdentity API. Its artifact grants name only `core.f_probe_<run_id>`
and the explicitly supplied real legacy artifact/action owner refs. Rights are
read/delete plus decrypt on those exact refs. Decrypt is required by the actual
protected plaintext GET path, not broad secret authority. Existing audit routes
also require unrestricted Retention read/update and Permissions manage. Those
powerful management rights are explicit isolated-fixture consent, never added to
the installer, invoker, process credentials or a production role. The provisioner
checks all three identities' effective grants through the public API. Installer
and invoker integration issuer provenance is checked separately. Human credentials go only to the provisioner; F gets private paths
for installer identity comparison, ordinary invoker and maintenance credentials.

The existing `FileLegacyAuditRepository::operator_id` guard intentionally requires
unscoped Access. Maintenance therefore uses canonical `/auth/login`, not an
integration exchange. `CreateIdentityRequest` supports an 8..128 character
password, and `create_identity` hashes it before repository creation. Current
`login` verifies it and calls `generate_access_token` without integration scope.
There is no force-password-change DTO field, mandatory notification or interactive
step on this supported path. The fixture does not invent attributes to bypass one.
It verifies maintenance's local provider metadata and actual identity/grants through
`/auth/me`, reauthenticates after the temporary issuer is frozen, then calls the
actual audit-capabilities endpoint with that fresh maintenance access token.
Denial fails without trying the human, issuer or integration token. No guard changes.

F's `maintenance_credential_file` now points to a new mode-0600
`maintenance-credential.private.json` containing exactly:

```json
{"kind":"password","role":"maintenance","run_id":"<run_id>","owned_project":"<project>","identity_id":123,"login":"acceptance.<normalized_run_id>.maintenance","password":"<private generated password>"}
```

These are the exact fields consumed by F's typed `password_login`. The identity
ID is returned by protected CreateIdentity, not a guessed fixture constant. F
validates run/project/role/login metadata, dispatches only login/password to
`/auth/login`, then checks actual `/auth/me` identity separation. Installer and
invoker remain raw integration credential files using `/auth/token-login`. No
maintenance integration token is minted. The maintenance password is never copied
from the supplied operator credential, logged in public evidence/plan, put on argv,
or sent to a worker, producer, provider or native inspector. It exists only in the
private file and in-memory auth/provisioning request bodies. The password account
has no artificial expiry claim; its lifetime is this disposable deployment.
Successful teardown unlinks its exact file and removes only the owned database
volume after joining writers. On failure, private state retains known principal
IDs for the operator's scoped recovery, not passwords or an invented rollback.

Additional private inputs are mandatory, not result flags:

- `legacy_fixture_file` contains run_id, project, backend, positive version_id,
  exact artifact_ref and action owner_ref from a real old writer. They must belong
  to this planned run. F's inspector must still observe actual legacy_unknown
  custody and quarantine denial. No rows, checksums or settlement receipts are
  fabricated. Arrange the actual legacy workload/data through its owning fixture;
  a declared ID alone proves nothing.
- `observer_credential_file` contains run_id, project, user, password, database
  and schema for a separately provisioned SELECT-only native observer. It cannot
  be the application/admin DB username. The DB fixture owner provisions that
  role; no Python SQL role creation or schema reset is supplied. The driver binds
  its credential to the owned PG port and F's repository inspector independently
  confines operations to bounded read-only transactions. A differently named user
  is not proof of SELECT-only grants; the DB fixture owner must establish them.

The inspection service must mount the same owned artifact volume as the API and,
for SharedFile, the same owned filesystem blob volume. Both profiles explicitly
list the possible action-writer services. ObjectSegments adds only a loopback
ephemeral port for the owned RustFS provider. Its mode-0600 provider file is
generated from the actual per-run bucket, prefix and generated provider keys, not
the maintenance/operator/process credentials. F independently verifies container,
volume and endpoint ownership and actual versioning. The driver removes the exact
private binding/provider/inspector files only after successful owned teardown.

Required source integration includes F's repository-backed probe, producer,
inspector example and repository module. The inspector binary is a real central
build prerequisite, not an external fixture implementation placeholder. Current
F source must also use supported root `/auth/token-login` and `/auth/me` paths;
`/api/v1/auth/*` is not registered by current `server.rs`. Source incompatibility,
missing immutable images, missing real legacy data, missing observer credentials,
or audit integration-token denial is failure. No live result follows from these
offline sender/provisioner tests.

## What the driver itself checks

For each profile it starts a new isolated PG18/transport stack with current
prebuilt images. It strips fixed container names, fixed host ports, explicit
network subnets and externally named resources. Only engine-assigned loopback
API/PG ports and the owned ObjectSegments provider port are exposed. Writable binds, anonymous volumes and external volumes
are rejected. DB credentials are generated for this owned PG server/migrator and
normal DB consumers, not inserted into stock worker/web environments or fixture
stdin. Fixture code is trusted operator code and can inspect its private Compose
file; that access is not permission to create application users through SQL.

SharedFile forces the filesystem blob provider into the owned `/opt/attune/blobs`
volume shared by API/executor/supervisor, and requires the API/action/sensor
artifact volume to match. ObjectSegments forces S3 to the owned provider service,
with a fresh per-profile bucket/prefix and run-generated RustFS credentials; it
never inherits a production S3 endpoint from a config mount. Its bucket initializer
receives `AWS_ENDPOINT_URL`, `ATTUNE_ACCEPTANCE_BUCKET` and the generated credentials.
Provide a RustFS-compatible fixture and a real versioning initializer. Neither
configuration assertions nor the `versioned_provider` manifest flag substitute
for the required physical probe/versioning observation.

Platform readiness must advertise the complete converted admission contract.
The current compiled gate being OFF is an actionable **failure**, not a success
because denial was observed. No individual release flag is toggled. A temporary
coordinated test build may be supplied only by the integration owner after its
prerequisite matrix is green; this runner does not manufacture that build.

Through the real token-login and protected pack APIs, the driver performs:

1. Fresh core publication pinned to the actual canonical archive digest.
2. A persisted `max_action_timeout=77` operator override.
3. Rejection of a real candidate with an unapproved external ceiling. It requires
   the covering-approval denial, not an unrelated parse/auth error, and compares
   exact release/policy/config snapshots afterward.
4. Atomic wrong-archive-pin rejection with unchanged persisted snapshots.
5. Successful ordinary upgrade adopting new authored defaults while preserving
   the override and retained in-flight execution.
6. Ordinary readmission of retained original bytes for rollback, with exact old
   release/digest and preserved override/restored defaults. No blind pointer edit
   or retry after an uncertain HTTP commit is performed.

Upgrade denial now checks the current actual separate-external-approval boundary:
HTTP 403 plus that exact reason, not generic Forbidden or the later compiler's
"covering approval" wording. The already accepted Rust fixture is not changed.
Every failed upgrade/wrong pin, successful upgrade and retained rollback writes a
new mode-0600 `*.outcome.private.json` before teardown. It records before/after
observable release/policy/config inventory, response disposition and an error
class only. It never logs raw secret-bearing HTTP bodies or retries publication.
A lost response with changed pins remains a failing run requiring review, not a
claimed rollback. Unavailable post-state stays unknown. A single unchanged read
after transport failure is not proof that no late commit can occur; the run still
fails and writers are joined. These files retain evidence even if the disposable
database is subsequently removed by verified owned teardown.

Then it **stops and waits every non-foundation container**, including any owned
probe/old-image orphan, before F's exact precompiled cases run on the owned PG18
server. F's own fixtures create physical pre-06005 clones, run the exact
writer-drained parking preflight and 06006 quarantine path, retain original
table/constraints/checksums, reconcile native PUT/cleanup obligations and test
event-ID twins with exact timestamps/partition expiry. The driver never copies
those SQL bodies, changes migration history, wipes schemas or guesses backfills.
These cases are not a claim that an arbitrary old production database has been
migrated; the live upgrade fixture's provenance still matters.

## Teardown and failure

The work root must not exist. Existing project resources cause refusal, not
adoption. Every Docker resource carries both the unique project and owner label;
every destructive step inspects both, including foreign resources that share a
project label. All foreground process groups are terminated/joined, and container
writers are stopped, awaited and inspected before preflight or volume cleanup.
`down` never uses `--volumes`. Only exact inspected owned volume IDs are removed,
after no containers remain. The work root is **never recursively deleted**.

Cleanup is awaited on normal failure and catchable interruption. An unjoined
writer, foreign label, unavailable daemon or incomplete stop makes cleanup fail
and retains DB/provider volumes. It does not trade lost evidence for a clean
status. SIGKILL/host failure cannot be trap-cleaned: retain `owner.json`, the
private Compose file and exact resource IDs for an operator-owned writer audit
before any subsequent scoped cleanup. There is no automatic adoption, age-based
janitor, TTL-as-PUT-settlement or manual unowned volume-reset command.

Private credential/Compose files are unlinked only after their obligations end;
non-secret lifecycle evidence and actual local archives remain in the owned
work root. A failed run is not silently retried with a new database. Successful
Compose profiles still report full SOW acceptance as **INCOMPLETE** until the
separate live matrices pass.

## Helm is separate

`scripts/acceptance/helm-lifecycle.prerequisites.json` describes the required
fresh/failed-required-upgrade/rollback matrix for both storage modes. Local
cluster provisioning and namespace ownership are external, explicit prerequisites.
The driver never selects or mutates a kube-context. An optional manifest `helm`
entry with absolute `chart` and `values` paths permits offline four-case rendering
and rejects content-dependent ordinary Pod startup. Render output is private and
is not live acceptance. With `helm --wait`, ordinary workloads must use platform
readiness before post-install publication and content hooks run.

The unreviewed Compose web gate is not edited or applied by this source. A
fixture may deliberately select reviewed topology, but cannot claim that a
different committed baseline gates content correctly.
