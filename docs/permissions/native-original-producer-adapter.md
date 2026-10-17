# Original native producer setup and inspection

`crates/common/examples/native_settlement_adapter.rs` is F's actual setup and
repository inspection executable for E's owning native lifecycle. It creates the
original producer and operation before the first native body request. It neither
inserts fictional historical `legacy_unknown` rows nor enrolls unmanaged history.
Production gates remain disabled.

## Persistence and authorization

Forward migration **20261016006015** adds immutable generation/operation records
and the distinct internal `controlled_native_v1` PUT protocol. Earlier migrations
are unchanged. A controlled native operation can be inserted only with the exact
original admission, execution attempt, allocated worker/instance, file reservation,
retained source receipt and mode-qualified destinations. It cannot acquire another
writer protocol or retire from an atomic sentinel or ordinary PUT observation.
Retirement requires the existing completed native audit, exact frozen member and
scope, and its immutable settlement evidence. Unknown legacy history retains its
existing quarantine path. These storage identities grant no public SecretOrigin.

The adapter requires a private mode-0600 context, explicit `--execute`, and
`enabled_for_controlled_acceptance: true`. The context binds the owned loopback
database, non-superuser setup role, exact schema/run/project/generation/mode,
maintenance identity, managed producer action, matching application JWT verifier
and encryption key, and pinned native management transport. No database URL,
member, source rows, grants, SQL or commands come from stdin.

The maintenance credential is a real, unscoped, live Access token from canonical
password login, verified against the owning API's actual JWT configuration.
Current repository IAM must authorize permissions management and retention
update/read. Creating the producer additionally requires Actions/Execute on the
actual action, Workers/Create and Artifacts/Create on the actual new artifact.
E provisions those bounded fixture rights through its normal installer. Runtime,
integration-scoped, worker, refresh and expired credentials are rejected. No
permission sets or approvals are fabricated or broadened by setup.

## Real original creation sequence

1. The already-running owning controller receives an authenticated manager POST
   to `/native-generations/{generation}/prepare-origin`. It checks its real
   mount/root/boot/UID/cgroup binding, unmanaged-process exclusion, empty roots,
   unfrozen empty writer leaf, empty request journal and irreversible fence state.
   It durably creates one original operation nonce and freezes its configured
   writer certificate/instance/attempt identity BEFORE any native request.
2. The controller signs a fresh nonce/request-digest/purpose-bound RS256 enrollment.
   This is original identity evidence, NOT stop/provider-settlement evidence. The
   adapter authenticates the pinned signature, scope, run/project/mode, freshness
   and exact request. It cannot construct the opaque proof from caller JSON.
3. `NativeFileSetupRepository` enters graph admission, authenticates current IAM,
   resolves the actual managed action/policy/executable, and creates a Requested
   producer with NULL config/result/worker. The original deployment-authenticated
   attempt nonce is attached before its first admission; grants, release, owner
   and attribution remain the actual selected policy. Reused attempts are denied.
4. Setup materializes, validates, attaches and reads back the complete compiled
   schema pair, initializes config exactly once with canonical secret redaction,
   captures original human input authority, seals accepted input, then assigns a
   real worker and makes the producer Running. Worker delivery is registered with
   the original authenticated instance and actual admission/attempt.
5. The actual accepted-input source is prepared and its exact purpose receipt
   held before artifact/version locks. A private file version is created Pending
   with the appropriate original object/staging destinations. Worker delivery and
   `FileOriginRepository::reserve` retain that original source and physical pins.
   No RuntimeReadParent or readable Pending sink is invented. Source, delivery,
   enrollment and JWT freshness are checked through the final commit/handoff.
6. Setup records the exact operation/member, then registers a real
   `controlled_native_v1` request. The native writer has not run yet. The file is
   deliberately Pending, not a fabricated successful body/Ready publication.
   Its provider version is genuinely NULL. Later inode-qualified catalogue proof
   supplies physical identities; HEAD absence cannot do so.

Blob, shared and combined modes are supported. Combined mode keeps both original
destinations in ONE member. The tiny source-only fixture pack at
`scripts/legacy_storage_controller/producer_pack` supplies the secret input
declaration. E installs it through its normal owned pack API/deployment so real
release, action, policy and schema records exist. It is not installed automatically
and contains no secret output. Alternatively use an actually deployed compatible
action with exactly the required `native_source` secret string input.

The native instance, worker instance and attempt UUIDs are authenticated original
deployment declarations in the controller's private writer certificate binding.
They become the real producer's original delivery identities at creation, not
updates to a historical execution. The controller mints the PUT operation UUID.
After setup returns, only the same enrolled request may enter that writer. Scope
and complete-member equality are enforced again by the normal application audit.

## Exact E executable interface

Build the matching `native_settlement_adapter` example centrally. Pin the resulting
regular executable and SHA256 in E's artifact receipt. Context schema is provided
in `scripts/legacy_storage_controller/producer-context.example.json`; placeholders
and its disabled gate must be replaced only inside the owned central fixture.

```text
native_settlement_adapter --execute
environment: ATTUNE_NATIVE_ACCEPTANCE_CONTEXT=/private/producer-context.json
stdin: {"protocol":1,"run_id":"owned-run","project":"eaccept-owned-run-native-blob_and_shared","mode":"blob_and_shared","operation":"prepare-original-native-producer"}
```

`generation` may be included and must equal the private context. It may be omitted
for E's already-staged lifecycle caller. The context's generation is always bound
to the authenticated controller proof. The initial result is:

```json
{"protocol":1,"data":{"member":{"operation":"actual UUID","version":123,"root_read":456,"object_key":"actual key or null","provider_version":null,"staging_path":"actual path or null","execution_attempt":"actual UUID","worker_instance":"actual UUID","native_instance":"actual UUID"},"artifact_id":7,"artifact_version":1,"execution_id":8,"admission_id":9,"worker_id":10,"source_receipt":11,"scope":{"backend":"actual digest","staging_prefix":".attune-upload/"},"protocol":"controlled_native_v1"}}
```

All IDs are actual repository results. No plaintext, ciphertext, secret parameter,
DB credential, JWT, permit token or caller-supplied proof is returned. An identical
setup retry returns the existing original member while publication is still open;
it does not reinitialize config, reseal input or start another execution. Closed
publication must use inspection, not setup again.

For the before/after observations E already calls:

```json
{"protocol":1,"run_id":"owned-run","project":"eaccept-owned-run-native-blob_and_shared","mode":"blob_and_shared","operation":"observe-original-native-custody","version_id":123}
```

Output `data` contains `member`, `scope`, `evidence_owner_count`, `pins`,
`unresolved_operations`, nullable `object_done`/`staging_done`, and `audit`.
The optional version argument must equal this generation's original version.
Inspection uses repository-only repeatable-read queries and current operator IAM.
It never trusts HEAD, an input PASS flag or the daemon launch result.

When an actual audit exists, `audit` includes its ID, state, complete frozen plan,
`plan_matches_original`, per-kind persistent receipt counts and up to 64 immutable
controller receipt identities with operation/request-digest/verification-key
bindings. These are actual stored observations, not observer-supplied booleans.
E can obtain the complete signed receipts through the existing paged operator API.
After real audit completion all three owner/pin/operation counts must be zero.
Quarantine must retain the original counts and original unresolved request.

## E integration and validation limits

E keeps ownership of packaging, credentials, topology and normal API request/step
orchestration. Include **six** controller modules now, including `http_input.py`,
and their updated frozen hashes. The server also enforces aggregate HTTP input
bounds and private key/GID isolation. Keep UID/GID 61124, no capabilities, separate
stable writer leaf and fresh native/worker/attempt UUIDs per generation.

The existing E lifecycle's prepare/observe operation names, output member and API
IDs match this adapter. It may continue posting its owned public acceptance body
and observing the real first-chunk barrier. That is an unfinished generic native
write, NOT a claimed successful protected GET or encrypted Ready publication.
Request and step the ordinary API audit; do not create audit evidence or update
PUT/owner/pin tables from E. Apply 06015 and the matching audit/controller/repository
source atomically before building the adapter. Use a scoped non-superuser DB role
with the needed repository table/function rights, not postgres/admin fallback.

The F live kernel harness also now obtains its member only from this pinned
adapter; its binding rejects caller `member`/`write` fields. Its full live lane
executes the nine original SQLite tests plus four real enrollment recovery tests,
then the real kernel/API failure matrix. Those runtime suites and live cases are
central-only. Source/mock checks, rustfmt and parsing do not establish acceptance.
No compiler, PostgreSQL/SQLite runtime, kernel, process, provider, service or
generation job was run while authoring this implementation. Cloud/untracked
history remains quarantined; no production gate or deployment is enabled here.
## Enrollment and application rollback

Enrollment and application setup use separate local transactions. There is no
cross-database atomic commit. The controller first reserves its original operation
nonce durably, before any body request. If application setup rolls back, retrying
the same run/project/mode/writer identity returns that exact nonce while the real
generation is still pristine, unfenced and has no admitted request. It does not
mint another operation, reset the journal or reconstruct old application rows.

If a writer was admitted or physical children appeared before that retry, setup
fails closed. Keep that generation and journal quarantined for owned diagnosis;
do not adopt those bytes as original source history or rewrite bindings. A changed
run/project/mode/certificate/attempt, reboot, mount replacement or fence also
prevents retry. A lost reply after application commit returns the immutable
original application member while publication remains open. It never starts the
writer again. Audit compares the application's full frozen member against the
writer journal; enrollment alone is neither an application commit receipt nor
proof of settlement.

The setup DB login must have direct object privileges in the exact private
context's owned database/schema. The adapter rejects superuser, CREATEDB,
CREATEROLE, REPLICATION, BYPASSRLS and every role membership, including predefined
file/process-access or administrative roles. E must not fall back to an admin
login or grant a broad role to satisfy setup. The adapter executes no caller SQL
and grants no database or application permissions. Current maintenance IAM checks
are additional to, not replacements for, the scoped database boundary.
