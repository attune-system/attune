# Owned native controller packaging and lifecycle

This disabled-by-default systemd package needs the owning host PID/mount namespaces,
ext4/XFS and a stable delegated cgroup-v2 writer leaf. Ordinary Compose volumes or
Helm PVCs do not establish those properties. Do not enable hostPID, capabilities or
Docker sockets on application services to imitate them.

## Artifacts and installation

After the earlier F units, integrate `26ea93de`, then `f9c476ee` and F's repository
export correction as prerequisite deltas, not E work. The frozen-source target
builds actual `legacy_native_writer`, `native_settlement_adapter` and E's canonical
`native_producer_acceptance_archive` examples. It exports all SIX Python modules,
including `http_input.py`, the systemd unit, disabled controller/context templates,
the original producer pack and its measured canonical archive. The receipt has
`binaries`, `native_controller_files`, and `native_producer_release`.

`scripts/acceptance/native-controller-source-pins.json` records F's supplied six
source checksums and forward 06015 checksum. Frozen archive extraction rejects
missing/mismatched modules/migration/repository/pack inputs before any build.
Export verification checks the actual modules, ELF architecture and actual
canonical producer archive checksum. Source hashes are NOT binary/image hashes.

After central releases the freeze and authorizes the existing local builder:

```sh
python3 scripts/prepare-bootstrap-artifacts.py --manifest /owned/acceptance/build.json --work-root /owned/acceptance/build-unique123 --execute --permit-build
```

Seven immutable bases and the measured frozen source/revision/time remain required.
No build ran while authoring this unit. Verify receipt hashes before installation.
Central installs the exported six `.py` files as root:root 0444 under root-owned
non-writable `/opt/attune/native-controller`, the executable root:root 0555 at
`/opt/attune/native/legacy_native_writer`, and the unit under `/etc/systemd/system`.
Never overwrite another run's installed package. The unit has no Install section,
no restart policy, no caps, no-new-privileges and UID/GID 61124. Reserve that UID
exclusively for this fixture. It intentionally adds no mount/PID namespace.
Install the measured adapter as a root-owned non-writable executable named
`native_settlement_adapter`, and the canonical producer archive as a root-owned
non-writable file. Their parent directories must also be root-owned/non-writable.
Copying actual bytes preserves their measured hashes; do not put source checksums
in the binary or archive digest fields.

For each mode `blob`, `shared`, `blob_and_shared`, use a separate new generation and
project `eaccept-RUN-native-MODE`. Install its config at
`/etc/attune/native-legacy/RUN-MODE/config.json`. TLS/signing files are root-owned
0440, group 61124, with root-owned non-writable ancestors. Writer and manager need
distinct pinned client DER hashes. Pin actual server cert, independent RSA signer
and writer hash. Never reuse an execution JWT signing key.
Before init/setup, bind the actual writer certificate DER fingerprint to fresh
canonical `native_instance`, `worker_instance`, `execution_attempt` UUIDs, and use
a fresh generation UUID. These are original deployment declarations, NOT existing
DB IDs or caller member fields. E checks the actual certificate mapping and denies
UUID reuse across lanes. F authenticates these declarations through manager mTLS
`prepare-origin` before its original producer transaction. E never calls that
route to manufacture an unsigned enrollment or adopts historical attempts.

Provision separate private 0700 parents owned by 61124 for future blob/shared
roots, which MUST NOT exist. Provision a separate persistent 0700 journal directory.
Delegate an empty stable cgroup-v2 leaf to 61124 outside the daemon unit subtree,
with cgroup.kill/freezer support. Init is exclusive; serve cannot init. Never rerun
init, replace roots, or delete/recreate a fenced cgroup. Both config and CLI gates
default off. Only central installs a per-instance drop-in for this controlled run:

```ini
[Service]
Environment=ATTUNE_NATIVE_ACCEPTANCE_FLAGS=--enable-controlled-acceptance
```

Central alone performs daemon-reload. Do not enable the unit at boot. Lifecycle
checks actual systemd properties/inactivity/config before init/start, then checks
a changed InvocationID with unchanged boot/root/journal/cgroup identities after
restart. It requires frozen=1/populated=0 to survive. Stop waits for MainPID=0.

## API binding and live flow

The isolated API needs the local provider's exact roots/mounts and JCS backend
fingerprint. Set `storage.legacy_audit_controller` using its HTTPS root, `issuer`,
`audience`, `verification_key_file`, `client_identity_file`, `ca_certificate_file`.
Mount only manager files read-only into the API, never writer credentials, signer
private key, cgroup or Docker socket. Compose/Helm can pass this ordinary config
only after central establishes the supported host topology independently. Their
ordinary volumes/PVCs are NOT attested. RustFS/S3/ObjectSegments settlement remains
a separate external-provider requirement; this controller must never attest it.

The source lifecycle plans without reading credentials or starting subprocesses:

```sh
python3 scripts/acceptance/native_controller_lifecycle.py --binding /owned/native/binding.private.json
```

Later central-only execution, foreground under a bounded scheduler deadline:

```sh
python3 scripts/acceptance/native_controller_lifecycle.py --binding /owned/native/binding.private.json --execute --enable-controlled-acceptance
```

The private root-owned 0600 binding contains `run_id`,
`enabled_for_controlled_acceptance`, new `evidence_file`, `controller_directory`,
`controller_sha256` mapping six module names to the reviewed source hashes,
`producer_adapter` with actual `native_settlement_adapter` binary/hash,
`producer_release` with actual `archive_file`/`digest`, and ordered `cases`.
Each case has `mode`, `project`, `controller_config`, `systemd_unit`,
`producer_context_template_file`, new `producer_context_file`,
`api_url`, separate `installer_credential_file`/`maintenance_credential_file`, `controller_endpoint`, `writer_ca`,
`writer_certificate`, `writer_key`, `server_der_sha256`. Inputs and ancestors must
be root-owned real files/directories without group/other writes.

The actual F adapter is now wired, replacing the earlier pending-interface note.
Use F's exported `producer-context.example.json` as the private context template.
Populate its exact run/project/generation/mode/schema/database/role, actual
maintenance identity ID and `f_native_settlement.producer` ref. The loopback DB/port
must be that isolated lane's real DB and its role must be a separately provisioned
non-superuser setup role with the repository/function rights required by F. This
is a write-capable setup adapter, NOT G's SELECT-only metadata inspector. Never
borrow app/postgres credentials or mix G/F contexts.

Provide actual JWT-verifier and encryption-key files as private root-owned 0600
files. F checks current Access-token/IAM and encryption config. The template's
manager transport must match exact local filesystem roots, endpoint, signer,
audience, CA and manager certificate. Template, new context and new access-token
file must share the same protected private directory. No secrets appear on argv
or in build/artifact receipts. Do not include operational contexts in the frozen
source archive.

Each lane needs its own inactive unit, fresh roots/journal/cgroup and separately
owned loopback API/database. E logs in the distinct local-password maintenance
principal, verifies `/auth/me`, installs F's real canonical producer pack through
ordinary multipart upload as the separate installer, and reads back the actual
managed action. There is no force/test suppression/direct registration/SQL path.
An uncertain upload response is recorded as unknown and not retried automatically.
Installer requires owned-pack install/read and action read. Maintenance requires
F's permissions-management, retention read/update, owned Actions/Execute,
Workers/Create and Artifacts/Create rights plus the ordinary metadata-delete
authority used by this flow. Provision these through normal protected IAM APIs;
neither E nor the adapter assigns an admin role or bypasses onboarding.

E writes a fresh private context with the actual password-login Access token, starts
its own daemon and waits for mTLS readiness. Then it foreground-awaits the pinned
adapter with protocol 1, explicit generation and `prepare-original-native-producer`.
F's authenticated prepare-origin enrollment and actual repository transaction
return original member/API/admission/worker/source IDs. E checks controlled_native_v1,
all positive actual IDs, exact scope, Pending provider version and exact original
writer UUIDs. It forwards the member unchanged. It never supplies member/write
JSON or fictional legacy_unknown rows. Forward 06015 and matching sources must
be migrated centrally first; this runner never migrates or runs SQL.

The flow holds a real writer with descriptors for all original
destinations, deletes metadata through the API, checks retained pins, requests the
ordinary frozen audit and compares its complete original member and exact scope.
Normal API steps drive fence/catalogue, same-boot daemon restart, exact delete and
settlement. Actual persisted writers/catalogue/absence/settled receipts and physical
repository pin/operation/owner release are mandatory. After each observation E
checks the original member/scope; final audit ID, completed state, frozen plan,
receipt counts and immutable signed-controller identities must match the real
API audit. Combined mode preserves both
obligations in one member. HTTP 202 or descriptors alone never prove settlement.

Success and failure retain roots, journal and kernel fence. SIGTERM/interrupt
attempts daemon stop. If the writer is not fenced, quarantine it; the central owner
must join remaining native writers and drain the API before removing any enclosing
storage. After successful audit, daemon join and populated=0, E unlinks only its
new context/token files. Failure keeps them private for recovery. Roots, journal,
kernel fence and all externally provisioned credentials remain retained. The
script never removes volumes. F's nine real SQLite and four enrollment tests, signed-client
tests, crash/cert/identity/provider negatives, native cache/wait tests, both normal
storage profiles and Helm live upgrade/rollback still require central execution.
Every result keeps `full_sow_acceptance` false.

Do NOT run F's `live_cases.py` concurrently on an E generation. The seven fresh F
failure lanes remain a separate central matrix with their own daemon ownership,
`--execute-owned-live` and mount-fault permission where needed. Missing kernel or
filesystem capability is nonzero unsupported, never skipped-green acceptance.
