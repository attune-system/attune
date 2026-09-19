# Platform catalog ownership

The platform catalog exists without an installed `core` pack. API startup calls
`PlatformCatalogRepository::reconcile` before it starts background writers or
accepts requests. The API migration and legacy-release upgrade commands also
reconcile before proceeding. Definitions are compiled from
`crates/common/src/platform_catalog/`; installed pack files are not an input.

This implements the ownership portion of the
[pack release specification](pack-release-artifacts-spec.md). It does not
implement component retirement, rollback, or the required-pack installer.

## Stored ownership

Migration `20260916000001_platform_catalog.sql` adds a stored PostgreSQL
`management_origin` enum to runtimes, permission sets, triggers, actions,
sensors, rules, policies, work queues, workflow definitions, dashboards, and
cache namespaces. Runtime versions inherit ownership from their runtime.

The generated column prevents disagreement with the existing pack association,
ad-hoc flag, and catalog revision:

- `platform` has a positive `catalog_revision` and no pack owner.
- `pack` has a pack owner and is not ad hoc.
- `ad_hoc` has no lifecycle pack owner. An ad-hoc row may still have a pack
  association for execution or display.

`ManagementOrigin` exposes these cases as a Rust enum through repository
ownership lookups. Use the ID lookup for scoped dashboards or cache definitions.
A ref lookup rejects ambiguous scoped refs. Namespace validation remains in the
loader and is independent of ownership.

`managed_release` records a pack projection's release. A composite foreign key
requires that release to belong to the same pack. Pre-release legacy rows may
have no release. The migration backfills existing active releases; component
creation and `PackReleaseRepository::activate` maintain the field. When the
retirement ticket changes activation, it must stamp only the incoming release's
projections rather than every surviving pack-owned row.

Migration `20260916000002_catalog_bootstrap_and_release_lock.sql` makes component
creation take a `FOR SHARE` pack-row lock before reading `active_release`.
Activation takes `FOR NO KEY UPDATE` on the pack before changing releases and
components; uninstall also locks the pack before its releases. An insert that
starts during activation waits and stamps the newly committed release. An insert
that gets the lock first must finish before activation sweeps the projections.
`FOR KEY SHARE` would not provide this serialization.

## Catalog revision 1

The catalog owns these exact refs:

- Runtimes `core.shell`, `core.python`, `core.nodejs`, `core.native`, `core.java`,
  `core.ruby`, `core.perl`, `core.go`, and `core.r`, including the bundled Python
  and Node.js version definitions.
- Permission sets `core.admin`, `core.editor`, `core.executor`, and `core.viewer`.
- Triggers `core.alert`, `core.queue_started`, and `core.queue_empty`.

`standard` remains reserved execution authorization behavior.
`core.key_creator`, core actions, and timer metadata remain pack-owned. The removed
`core.ask` action and `attune.inquiry/v1` intrinsic handler are not catalog entries.

Workers register the native executor independently of database interpreter
registration and advertise `native` even with an interpreter-only runtime
allow-list. Native execution does not depend on an installed interpreter.

Reconciliation locks the schema-local singleton catalog-state row, checks the
compatibility epoch and monotonic revision, and commits every definition and
ownership transfer together. It rejects newer stored revisions. A repeated
revision leaves component metadata and timestamps unchanged. Reconciliation also
fills the exact compiled source-definition snapshot in
`platform_catalog_state.bootstrap_definitions`, introduced by the second
migration. This does not change the catalog revision. On a revision change it locks the
affected metadata tables before checking ownership, so a concurrent first insert
cannot evade the conflict check.

The first reconciliation transfers only the exact refs above from the pack row
whose ref is `core`. It also checks the cached pack ref, rejects auto-detected
runtime claims, and rejects sensor-bound system triggers. Ad-hoc rows and rows
owned by another pack abort the entire reconciliation. Existing IDs, permission
assignments, and references to those IDs remain unchanged. A `core.*` prefix by
itself never authorizes a transfer.

Runtime adoption checks the entire version inventory while the runtime-version
table is locked against writers. Unexpected versions or mismatched cached runtime
refs abort the transaction for operator review. The reconciler does not delete
children to make adoption succeed. Expected version rows retain their IDs and
host verification state while receiving the catalog's execution definitions.

Database triggers reject ordinary updates and deletes of platform rows and
runtime-version definitions. Runtime verification may still update `available`
and `verified_at`. The reconciler's transaction-local `attune.catalog_write`
setting is an application invariant, not authorization against someone who
holds arbitrary SQL access.

Pack cleanup selects lifecycle pack ownership. Transferred platform rows have
no pack foreign key, so uninstall cannot cascade into them. The Rust loader and
the bounded Python bootstrap bridge accept exact bundled platform YAML as a no-op and reject altered
definitions. This exception exists for bundled-core upgrades, not for arbitrary
platform overrides. New external releases must omit those files. Remove the
exception and its bundled-file parity test when bundled-core support is removed.
This exception preserves database ownership during source bootstrap. It does not
read older archive formats or adapt superseded index formats. Canonical releases
and indexes use the single initial v1 contract.

## Coordinated migration

This is a maintenance-window change, not a rolling upgrade. No part of this
procedure was executed against a deployed cluster as part of development.

1. Back up the database and record built-in component IDs, permission assignments,
   and external action/rule references. Inventory every database writer and its
   credentials, including migration jobs, bootstrap jobs, direct loaders, and
   scheduled maintenance.
2. Pause pack writes. Stop old APIs, workers, execution services, sensors,
   supervisors, and database-writing bootstrap jobs. Disable automatic restarts
   and scheduled jobs. Drain active work according to the deployment's maintenance
   policy before stopping execution services.
3. Disable login for every old writer role and revoke its credentials at the
   secret provider. Terminate existing sessions for those roles from the
   maintenance connection. Changing a password or revoking `CONNECT` alone does
   not terminate existing sessions; `PUBLIC` privileges can also defeat a simple
   per-role `CONNECT` revocation. Verify that no old writer sessions remain and
   that old credentials cannot reconnect.
4. Provision distinct writer credentials only to the compatible deployment.
   Keep schema ownership and migration privileges on a separate maintenance
   identity. Do not give old bootstrap jobs access to the replacement secrets.
5. Apply migrations through the deployment's existing migration runner. Do not
   switch between Docker filename history and SQLx history. With the SQLx runner,
   `attune-api --migrate` applies the migration and reconciles. With the Docker
   runner, apply the migration container first, then start the compatible API to
   reconcile before serving requests.
6. If reconciliation reports a conflict, keep writers stopped. Investigate the
   reported owner and resolve the claim explicitly. Do not delete assignments,
   clear ownership columns, reset the database, or force a takeover by prefix.
7. Verify compatibility epoch `1`, catalog revision `1`, all sixteen component
   refs, the inquiry handler, preserved IDs, and grants. Bootstrap the initial
   identity using catalog-owned `core.admin`, then install core through the API.
8. Start only compatible services with the replacement credentials. Keep the old
   credentials disabled. Retain the backup and recorded IDs for operator review.

New `Database` connections check an existing epoch/revision and fail closed on
an incompatible value. Migration connections can open databases that do not yet
have the catalog table. **The epoch does not fence old binaries.** Old binaries
never read it; credential revocation, session termination, schema privileges,
and deployment controls provide that protection. Do not roll back only the
binary after this migration.

## Source bootstrap

`scripts/bootstrap_core_pack.py` no longer runs the direct Python pack loader.
It waits for the API, creates its temporary identity using `core.admin`, and
uploads the bundled core content through the existing API path.

The volume-mode `docker/init-packs.sh` continues to invoke the updated
`load_core_pack.py`; existing Compose and shared-volume Helm ordering is retained.
Before the first API reconciliation it creates pack-owned source definitions.
After reconciliation, repeated runs compare each declared built-in against the
catalog's persisted source snapshot and return its existing ID without writing
the row or its runtime-version children. Changed definitions, ad-hoc claims,
foreign pack owners, and namespace mismatches fail and roll back the load.
Comparison uses JSON types, so a boolean changed to an integer is not an exact
definition match.

The loader checks the compatibility epoch/revision and holds a shared catalog
state lock for its entire transaction. Reconciliation holds an exclusive lock on
that same row before locking component tables. Neither path can check ownership
against one catalog revision and write against another. The loader never enables
`attune.catalog_write` or changes platform ownership. Deploy the updated loader
with the compatible services; old loader credentials must still remain revoked.

This bridge remains only until source bootstrap is replaced by the required-pack
installer. It does not implement that coordinator or change the wire formats.

## Verification

Use the separate local `attune_test` database with PostgreSQL 16 and TimescaleDB
2.17 or newer. Each integration test owns and removes its schema.
Install TimescaleDB into `public` before application migrations so its functions
are visible to each test schema. The Python boundary tests require `psycopg2-binary`
and `PyYAML`; set `ATTUNE_TEST_PYTHON` to a virtualenv interpreter if they are not
installed for `python3`. `ATTUNE__DATABASE__URL` can select a disposable test
database instead of the default local `attune_test`.

```sh
make db-test-setup
cargo test -p attune-common --lib platform_catalog::tests
cargo test -p attune-common --test platform_catalog_tests -- --ignored --test-threads=1
cargo test -p attune-common --test platform_catalog_boundary_tests -- --ignored --test-threads=1
```

The integration tests cover fresh and concurrent reconciliation, repeat
idempotency, an actual pre-migration schema with existing grants and references,
ownership conflicts, platform mutation guards, legacy core refresh, omission,
first release activation, uninstall, and newer revision/epoch rejection. Boundary
tests run the real Python bootstrap repeatedly after reconciliation and reject
changed built-ins. A two-connection test pauses activation after its projection
sweep, proves that insertion waits for its commit, and then runs release retention
to prove the inserted component does not pin the previous release.
