# Offline rollout administration (work in progress)

`attune-rollout-admin` is a separate one-shot executable shipped beside the API
binary in the Linux service bundle/API image. It does not link API startup,
load service configuration, use an API profile/token, listen on a socket or spawn
helpers. The API image's default CMD remains unchanged. Run this executable only
as an explicitly invoked offline deployment process/Job; this work does not
create such a Job or deploy a caller. Possessing the binary grants no database
authority. All seven recorded-consumer gates remain false.

## Deployment-owned inputs

Deployment must provide the exact expected roles, instance/boot UUIDs and immutable
artifact/image release digest. The caller never discovers or approves processes,
replicas, health, boots, release identity or orchestration state. Each restart
requires a newly staged authoritative generation under the exact-boot model.
Stage and activation are deliberately separate commands.

All input files are regular, non-symlink, single-link files owned by the invoking
OS user with mode exactly 0600. Credential files are bounded to 16 KiB, receipt
files to 16 KiB and manifests to 2 MiB. Inputs are opened once with no-follow and
nonblocking semantics and read from that FD, with identity/metadata revalidation;
FIFO/device/socket, hard-link, replacement and insecure-mode inputs are denied.
Receipt output must be in an existing owner-0700 directory. Publication is atomic
and refuses existing paths, with file and parent-directory fsync. Nothing writes
credentials to argv, inherited environment, profile state, manifests or receipts.
Never place URLs/passwords in command arguments, file names or job environment.
Provision credential files through the deployment secret manager as private files.
Projected-secret symlinks and root-owned shared mounts are not exempt: externally
materialize a regular owner-0600 file or an equivalent stable private bind mount.

The credential JSON has exactly these fields:

```json
{
  "database_url": "postgresql://OFFLINE_LOGIN:SECRET@DB_HOST:5432/DB_NAME?sslmode=verify-full",
  "installation_schema": "attune",
  "installation_id": "DBA_PROVIDED_INSTALLATION_UUID",
  "database_oid": 12345,
  "schema_oid": 12346,
  "capability_role": "DBA_PROVIDED_INSTALLATION_CAPABILITY_NAME",
  "runtime_roles": ["API_LOGIN", "EXECUTOR_LOGIN", "NOTIFIER_LOGIN", "WORKER_LOGIN"]
}
```

This is a schema example, not usable credentials. DBA/IaC obtains the complete
installation identity from `recorded_rollout_installation()` on its trusted
installation connection and secrets the exact values with the URL. The URL
must explicitly include user, password, host and database (no pgpass fallback); only `sslmode` query options
are accepted. Startup `options`/`role`/service-file/environment fallback is not
supported. Username must match both `session_user` and `current_user`. Linux
parent non-dumpability is applied before credential read; the one-shot process sets
umask 077 before creating files. The environment is
cleared before the current-thread runtime is constructed. The tool uses one
direct connection with a unique offline application identity, bounded connect,
statement, lock, operation and close timeouts, SQLx-owned transactions, and no
statement/payload logging. Errors contain fixed redacted codes, not driver or
JSON diagnostics. Database URLs, passwords, receipt nonce and full receipt are
never printed. Environment clearing does not promise to erase the original
kernel/launcher environment memory; launch the Job with a minimal environment.

## Commands

```text
attune-rollout-admin validate --manifest /private/manifest.json
attune-rollout-admin status --credentials /private/database.json
attune-rollout-admin verify-provisioning --credentials /private/database.json
attune-rollout-admin stage --credentials /private/database.json --manifest /private/manifest.json --receipt /private/staged.json
attune-rollout-admin activate --credentials /private/database.json --receipt /private/staged.json --confirm-generation PUBLIC_GENERATION_UUID
attune-rollout-admin revoke --credentials /private/database.json --receipt /private/staged.json --confirm-generation PUBLIC_GENERATION_UUID
attune-rollout-admin revoke --credentials /private/database.json --receipt-row PUBLIC_AUDIT_ROW_ID --confirm-generation PUBLIC_GENERATION_UUID
```

`validate` canonicalizes exact v1 membership without connecting to PostgreSQL.
`status` reads the bound installation and exact prior pointer/revision, including
bootstrap NULL/0. It makes no readiness assertion. Mutating commands first verify
provisioning. The runtime-role list is an explicit deployment-owned inventory;
verification does not discover whether the list covers a real deployment.

Stage locks the singleton, stages the exact manifest and issues a DB-owned
receipt in one transaction. The receipt retains installation UUID and database/
schema identity, random nonce, operation/version, exact prior ID/revision,
target ID/generation/digest/snapshot and issuing session/backend/time. Its maximum
lifetime is fifteen minutes, set by the database. File content is never authority:
activation compares it to immutable PostgreSQL state and separately validates
current installation, target, prior CAS, expiry and terminal state. Copying or
editing a file cannot change authoritative fields. Activation and one-way
consumption commit together. Revocation is a separate one-way terminal record.
There are no FKs from accepted authority/work to this metadata.

A local output failure after stage commit leaves a harmless unused receipt and
staged generation. Never auto-activate or overwrite a receipt. An authorized
operator may explicitly revoke the receipt, or use its public audit row ID
obtained through the restricted DBA/operator audit channel if the file was lost.
The audit row ID is not the random receipt nonce or a bearer credential. Reissue
with a new receipt output path by staging the same exact manifest. The original
unused receipt remains revocable and expires; there is no implicit adoption.

Any post-commit acknowledgement error has an uncertain outcome. Read status and
inspect the deployment's audit records. A lost activation acknowledgement may be
retried only to obtain a consumed/replay denial; the tool never turns that into
an idempotent success or mutates the prior revision from local observations.

## DBA/IaC provisioning contract

No migration or caller creates logins, grants membership or fixes ACLs. The DBA
must establish an installation-owned role inventory in IaC state (including exact
role OIDs/names and ownership). Creation requires cluster-wide absence. On repeat
provisioning, validate the exact owned role and every attribute, membership and
ACL. Refuse an existing name with no ownership proof or any conflicting
attributes/grants; do not silently adopt, ALTER or repair it. Use quoted
identifiers/parameterized trusted tooling, not pasted password-bearing SQL.

- A dedicated schema owner is NOLOGIN, NOSUPERUSER, NOCREATEDB, NOCREATEROLE,
  NOBYPASSRLS. It owns the installation schema, tables/sequences and functions.
  Installation schema CREATE ACL is owner-only; earlier searched `pg_catalog`
  is not service-creatable. Privileged functions pin `pg_catalog`, the trusted
  installation schema, then `pg_temp`, with no caller-controlled search path.
- Only the offline migrator may assume the schema owner, through an explicitly
  managed SET membership. Runtime and rollout logins must not be schema/database
  owners or members of those owners, including NOINHERIT paths allowing SET ROLE.
- API/executor/notifier/worker logins have only required application DML. They
  have no superuser/CREATEDB/CREATEROLE/BYPASSRLS, schema CREATE/ownership,
  direct/transitive rollout capability membership, or rollout mutation EXECUTE.
  Do not grant them the private offline receipt/terminal tables or sequences.
- DBA creates the installation-specific NOLOGIN capability named by
  `recorded_rollout_operator_role()`. It has no elevated attributes or parent
  memberships. Do not substitute a different role or create one from a service.
- DBA creates/secrets a dedicated offline rollout LOGIN with no elevated
  attributes. Grant direct membership in that capability without ADMIN OPTION
  (prefer INHERIT FALSE, SET FALSE). No other role memberships are permitted.
  Grant schema USAGE and SELECT on `recorded_rollout_generation`,
  `recorded_rollout_active`, `recorded_rollout_installation_identity`,
  `recorded_rollout_offline_receipt`, `recorded_rollout_receipt_terminal`.
  Grant EXECUTE only on `issue_recorded_rollout_receipt(jsonb)`,
  `consume_recorded_rollout_receipt(jsonb)`, `revoke_recorded_rollout_receipt(jsonb)`
  and restricted `recorded_rollout_admin_ledger()`. Do not grant direct table
  writes, sequence rights, schema CREATE, ownership or direct migration-7
  stage/activate EXECUTE. The wrappers call those functions as the trusted owner;
  session-user authorization still applies. Pure installation helpers are not
  mutation capabilities.

This initial caller requires a SQLx-owned ledger in the installation schema,
owned by the trusted schema owner. Docker/manual migration ledgers are not
converted, synthesized or treated as equivalent proof. Choose the SQLx offline
migrator explicitly for this profile; other runner profiles remain unsupported.

`verify-provisioning` is read-only and fails closed on identity mismatch, missing
or extra relevant grants, inherited operator authority, unsafe role attributes,
unsafe owner/CREATE ACL/function ownership/search_path, a missing/changed/extra
migration ledger, or a supplied runtime role with forbidden capabilities. It does
not create/adopt roles, alter ACLs, migrate or enable a service. The ordinary
Compose development shared-owner URL is **not production proof** and must fail
this verification; it is not silently exempted.

For rotation, quiesce and join the exact old offline Job/connections first. DBA
creates a new globally owned login and validates its attributes/ownership before
use. In one DBA transaction, grant the new direct capability/USAGE/SELECT/EXECUTE
set, revoke the old capability membership and every old rollout privilege, and
commit. Secret the new credential file atomically through the external secret
manager; never edit credentials in an active process. Validate with the new login
and prove old login denial. Drop the exact old role only after its owned sessions
are gone and IaC dependency checks pass. Do not use prefix/age cleanup. The
immutable receipt actor records the issuing login; a currently authorized new
operator can explicitly consume/revoke retained intent, subject to the same
installation/CAS/expiry fences. Rotation never rewrites receipt evidence.

## Validation and limits

Local fixtures use run-owned databases, separately authenticated real login
sessions, exact cluster-role ownership manifests and awaited teardown on success
and failure. Timing fault controls use the existing owned plaintext PostgreSQL
relay with synthetic secrets only; they are not deployment transport. Expiry
fixtures age private metadata using DBA-only trigger bypass in their disposable
database, rather than add a production clock override or wait fifteen minutes.

Packaging smoke checks validate the real one-shot binary in a copied API bundle
layout and source Docker packaging/default CMD. They do not prove an OCI image
build, architecture portability or a deployed provisioning profile. No service
observations, health probing, rollout aggregation, readiness conversion,
scheduler/admission/dispatch integration, queue E2E or gate changes belong here.
