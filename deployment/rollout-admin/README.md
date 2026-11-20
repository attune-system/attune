# Explicit offline provisioning and invocation lane (WIP)

The separately approved [candidate cache runtime profile](cache-runtime-profile.md)
adds an explicit offline DBA supplement and standalone Compose profiles. It has
owned host PostgreSQL evidence only; migration 9 remains outside the ledger and
the target Compose profile is not accepted. The original rollout provisioning
transaction below retains its existing conflict checks and defaults.

The original rollout assets do not make current production role grants compliant. They
never create/adopt roles, transfer ownership, change startup/defaults, migrate,
activate a queue gate, or deploy anything. All seven gates remain false.

## DBA ownership and configure/rotation

A DBA/IaC system must first own the exact existing role **names and cluster OIDs**.
Do not derive an inventory by finding a role with a convenient name. Credentials
and role creation/secret rotation stay in the external DBA secret manager. Keep
owner, migrator, cache DDL owner, runtime logins, rollout capability and rollout
operator distinct. Schema ownership and SQLx migrations must already be correct.
The dedicated installation owner and cache owner/capability are NOLOGIN; the
migrator can SET the installation owner through one direct, non-inheritable,
non-admin membership. Runtime roles may not assume either owner or the capability.
The cache owner is a separately inventoried DDL principal, **not granted to the
runtime by these assets**. This does not implement a cache DDL broker or ownership
transfer. A future separately approved DDL separation design is required.

Run `inventory.sql` on an explicit DBA connection to capture exact live role
attributes/OIDs, membership options, schema/table/column/function ACLs and function
definitions. Output is DBA custody, never public health or readiness evidence.
The repository cannot know a production installation's actual grants without that
inventory. Do not infer them from Compose or a test fixture.

`provision.sql` is an explicit transaction body, executed only through the offline
DBA driver `scripts/provision-rollout-admin.py`. Driver dependency: psycopg 3 and
Linux. Install that dependency into a DBA-managed environment; the API image and
service do not run it. Use a minimal environment and an explicit regular,
single-link, owner-0600 credential file containing only `database_url` (explicit
user/password/host/database; optional sslmode). No credential in argv/env. The
DBA session must be a direct superuser session, never SET ROLE. Inputs have
bounded no-follow FD checks; errors are fixed redacted codes. No helper processes
or role creation occur. A lost commit acknowledgement is an unknown outcome;
inspect exact state, never blindly repair it.

Inventory is owner-0600 JSON with exactly these fields (replace placeholders):

```json
{
  "version": 1,
  "operation": "configure",
  "schema": "attune",
  "installation": "INSTALLATION_UUID",
  "owner": {"name": "install_owner", "oid": 10001},
  "migrator": {"name": "offline_migrator", "oid": 10002},
  "cache_owner": {"name": "cache_ddl_owner", "oid": 10003},
  "operator": {"name": "offline_rollout", "oid": 10004},
  "capability": {"name": "INSTALLATION_CAPABILITY_NAME", "oid": 10005},
  "runtimes": [{"name": "runtime_api", "oid": 10006}],
  "previous_operator": null
}
```

```text
python3 scripts/provision-rollout-admin.py --dba-credentials /private/dba.json --inventory /private/owned-roles.json
```

Configure grants only schema USAGE, the five rollout tables' SELECT, four
receipt-wrapper/ledger EXECUTEs and direct non-admin/non-inheriting/non-SET
capability membership. It rejects changed OIDs, unsafe attributes, unknown roles,
extra relevant grants/memberships, schema CREATE, or runtime rollout privileges.
It does not repair conflicts. Identical configure is idempotent; follow it with
`attune-rollout-admin verify-provisioning` as the real operator.

For rotation externally create/secret and own a new login first. Quiesce/join the
old Job and all old login sessions. Supply operation `rotate`, the new `operator`,
and exact old `{name,oid}` as `previous_operator`. The one transaction refuses a
live old session, revokes the old capability/schema/table/function rollout grants
and grants the new set. It does not alter passwords or drop roles. Old login
retirement is a separate DBA action after exact absence/dependency verification.
Never prefix/age-clean shared roles. The existing active singleton row serializes these configure/rotate transactions.
Use external IaC locking for role/password changes; do not race external DBA
mutations against these transactions.

## Opt-in Compose

Standalone `compose.yaml` has only profile `offline-rollout`, no dependencies,
no service environment, no automatic migration, healthcheck disabled and no
restart/retry. Supply an immutable API image digest; do not use the default
shared-owner Compose database credentials. Run on an existing SQLx-ledger
installation reachable through its explicit network URL. The bind sources must
already exist: input directory UID1000 mode0700 containing regular UID1000 0600
`database.json` and `manifest.json`; separate receipt custody UID1000 mode0700.
Bind source auto-creation is disabled. No operator credential is mounted in an
API/executor/worker/notifier service. Command overrides remain one-shot:

```text
docker compose -f deployment/rollout-admin/compose.yaml --profile offline-rollout run --rm rollout-admin verify-provisioning --credentials /private/database.json
docker compose -f deployment/rollout-admin/compose.yaml --profile offline-rollout run --rm rollout-admin stage --credentials /private/database.json --manifest /private/manifest.json --receipt /custody/unique-stage.json
docker compose -f deployment/rollout-admin/compose.yaml --profile offline-rollout run --rm rollout-admin activate --credentials /private/database.json --receipt /custody/unique-stage.json --confirm-generation TARGET_UUID
```

Image/directory interpolation variables are nonsecret. No image/DB/PVC/service is
created by the files. Back up and retire receipt custody through the deployment's
private audit policy; never log or place receipts in ConfigMaps.

## Opt-in GKE/Kubernetes one-shot Job

`scripts/render-rollout-job.py` emits Job JSON only (no kubectl/API calls). Require
immutable API image digest, existing namespace, external input Secret and receipt
PVC. Separate stage and activate Jobs have unique names, backoffLimit0,
restartPolicyNever and 120s deadline; automatic retry is unsafe for lost ACKs.
No service account token or service-link environment is mounted. UID1000,
dropALL, no privilege escalation, read-only root and RuntimeDefault apply.

The Secret holds `database.json` and `manifest.json`. A bounded nonroot init
container using the same immutable API image copies the projected files with
`install -m 600` into private memory-backed emptyDir/input mode0700. Only init
mounts projection; main mounts the regular private copies read-only, never the
projected symlinks. Init cannot access receipts. The main directly starts only
attune-rollout-admin, not a shell/API server.

Externally prepare the PVC root group1000 with kubelet-compatible mode2770 and
its `custody` subdirectory UID1000/GID1000 mode0700. OnRootMismatch avoids
recursively broadening that already-prepared private directory; the caller fails
closed if ownership/mode differ. No init container chmods an existing receipt
volume. PVC/storage support, UID ownership, GKE networking/DB TLS, Secret/RBAC and
namespace isolation must be validated by the deployment owner. Do not share this
PVC/Secret with application Pods. TTL deletes Job objects, **not receipts**;
receipt custody survives stage→activation and failed/lost ACK for audit/revoke.

```text
python3 scripts/render-rollout-job.py --name UNIQUE_STAGE --namespace OWNED_NAMESPACE --image REGISTRY/API@sha256:DIGEST --input-secret OWNED_INPUT --receipt-pvc OWNED_CUSTODY --receipt-name unique-receipt --operation stage
python3 scripts/render-rollout-job.py --name UNIQUE_ACTIVATE --namespace OWNED_NAMESPACE --image REGISTRY/API@sha256:DIGEST --input-secret OWNED_INPUT --receipt-pvc OWNED_CUSTODY --receipt-name unique-receipt --operation activate --confirm-generation TARGET_UUID
```

Rendering is not applying. Exact boot IDs/release digest must come from deployment;
no asset discovers replicas or self-elects boots. SQLx ledger remains mandatory;
Docker/manual ledger conversion is not attempted.

## Owned recorded-queue validation

The receipt-only provisioning above does not grant consumer publication or
enable managed-queue admission. The owned acceptance runner separately installs
the candidate PostgreSQL observation transport as its NOLOGIN installation owner
and binds a distinct runtime login to each staged role, instance and boot.
Runtime logins cannot stage or activate a rollout. The services load the
operator-assigned identity from `ATTUNE_RECORDED_CONSUMER_IDENTITY_FILE` and
publish only their local observation. An observation cannot add a member or
replace a process boot in an existing generation.

The host lane uses an isolated SQLx ledger and test-specific DML grants. Default
Compose and Helm startup have no consumer identity files, reporter bindings or
active expected membership. A target deployment needs its own least-privilege
grant inventory, one distinct login and exact identity file per expected member,
and an explicit installation and receipt activation sequence. An exact-boot
process replacement requires a new staged generation. The queue admission gate
still requires every expected current observation. See the
[managed-queue acceptance procedure](../../docs/testing/running-tests.md#managed-queue-acceptance-on-owned-services).

## Unfrozen cache DDL candidate

The separately requested owner-controlled cache creator is only a
[candidate investigation](cache-candidate-status.md), outside migrations and not
operational. PostgreSQL requires the target leaf owner to have schema CREATE;
the original owner-only installation ACL blocked ALTER OWNER. The approved
[Option A experiment](cache-option-a.md) tests an uncommitted transactional
GRANT/ALTER OWNER/REVOKE without changing committed ACLs. No production grant,
verifier exception, role adoption or default change has been added to resolve it.
The test-posture census must be reviewed before any broad harness changes.
