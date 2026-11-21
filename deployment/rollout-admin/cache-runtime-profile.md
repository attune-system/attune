# Explicit cache runtime profile (candidate WIP)

This opt-in profile exercises the candidate cache DDL separation. It does not
change standard Compose, API startup, migration history or any readiness gate.
Candidate 9 remains `cache-partition-creation.candidate.sql`, outside the ledger.
Do not deploy it as a canonical migration. Queue forward migrations 1–8 and the
rest of the recorded SQLx history remain unchanged.

The ordinary development API currently owns cache DDL. Those shared-owner
credentials fail strict rollout verification. This profile instead requires
externally provisioned, separately inventoried principals:

| Principal | Posture |
| --- | --- |
| Installation owner | NOLOGIN, trusted schema/function owner; sole direct cache-owner membership with INHERIT/SET true and ADMIN false, for leaf ALTER OWNER only |
| Offline migrator | LOGIN; direct installation-owner membership with INHERIT false, SET true, ADMIN false; assumes owner only on the separate migration connection |
| Cache owner | NOLOGIN, owns existing parent and all leaves; no parent memberships or persistent schema CREATE |
| API runtime | LOGIN, no memberships or DDL; explicit cache DML/sequences and generation-only guarded EXECUTE |
| Maintenance | Separate LOGIN, cache-owner INHERIT true, SET false, ADMIN false; cache cleanup DML/read dependencies, DROP/ANALYZE as effective cache owner; no schema CREATE |
| Rollout operator | Separate LOGIN, existing receipt-only capability and grants; no cache role or runtime credentials |

No asset creates/adopts roles, transfers existing ownership, or repairs conflicts.
The DBA must verify installation/database/schema identity and exact cluster role
OIDs from its owned inventory, not choose matching names opportunistically.
Existing pre-9 cache parents and populated leaves must already have the correct
cache owner. Mismatches deny without adoption. The parent/leaf owner is preserved.

Run the original rollout configure transaction before the candidate cache
supplement. For subsequent configure or operator rotation, add
`"cache_runtime_profile": true` to the **rollout** inventory documented in the
[rollout provisioning guide](README.md), keeping the same exact cache-owner
identity. Use the original driver invocation for that inventory. The
`--cache-runtime-profile` switch selects the separate cache supplement.

The rollout transaction requires the installed candidate and validates the exact
owner membership, guarded creator, and cache parent and leaf ownership. The
cache supplement alone grants cache privileges. For rotation, stop and join
the old operator processes before running the transaction, then run
`verify-provisioning` as the replacement operator. Verification still applies
the same strict runtime and operator checks.

The offline DBA driver requires Linux and psycopg 3. Give it two separate regular,
single-link, owner-0600 files in an owner-0700 directory: DBA credentials containing
only `database_url`, and this inventory (all values come from external custody):

```json
{
  "version": 1,
  "installation": "EXACT_INSTALLATION_UUID",
  "database_oid": 10000,
  "schema": "attune",
  "schema_oid": 10001,
  "owner": {"name": "installation_owner", "oid": 10002},
  "cache_owner": {"name": "cache_owner", "oid": 10003},
  "api": {"name": "runtime_api", "oid": 10004},
  "migrator": {"name": "offline_migrator", "oid": 10005},
  "maintenance": {"name": "cache_maintenance", "oid": 10006}
}
```

`maintenance` may be null to provision API alone. The driver uses one direct DBA
transaction, validates the exact installed SQLx checksums against repository
migrations, and commits grants only if all checks pass. No credential goes in
argv/environment; output contains only a fixed result code. A lost commit
acknowledgement requires inspecting exact state; never blindly repair it.

```text
python3 scripts/provision-rollout-admin.py --cache-runtime-profile --dba-credentials /private/dba.json --inventory /private/cache-roles.json
```

API grants cover cache tables/sequences and read dependencies needed by cache
pin/lifecycle operations, plus platform catalog compatibility reads. This is a
**cache supplement**, not an inventory of every API domain's ordinary DML grants.
Full API configuration, MQ and storage remain external prerequisites. The strict
rollout verifier is unaffected. The database-owned intent still requires the
normal admission → parent → namespace lock order; raw creation calls deny.

## Opt-in Compose invocation

`cache-runtime.compose.yaml` is standalone and defines separately selected
`cache-sqlx-migrate` and `cache-api` profiles. It mounts only that process's private
configuration directory, never the DBA/operator credentials. It creates no host
paths and has no automatic dependencies, provisioning, migration, or restarts.
Supply an immutable API image digest and private uid-1000 directories containing
mode-0600 `migrator.yaml` or `api.yaml`. API config must select the ordinary API
login; migrator config selects the offline migrator and owner-assumption options.
The private configuration contract is the existing API configuration format.
Configure required service storage mounts/networks explicitly before full API use.

```text
docker compose -f deployment/rollout-admin/cache-runtime.compose.yaml --profile cache-sqlx-migrate run --rm cache-sqlx-migrate
docker compose -f deployment/rollout-admin/cache-runtime.compose.yaml --profile cache-api run --rm cache-api
```

These commands are examples, not an execution claim. The migration job invokes
the existing one-shot `attune-api --migrate` (packaged as `attune-service` in the
API image), with SQLx history; it never invokes
`docker/run-migrations.sh`. That Docker runner has a different ledger. Existing
Docker-managed databases must be rejected, never implicitly converted. Fresh
SQLx bootstrap also needs the offline migrator/owner's explicit runner-marker
permissions in `public`; grant those only to the offline installation posture,
never to API. Do not give the runtime owner credentials to make startup pass.
Run catalog reconciliation with the offline migrator. The API's startup check
can validate the exact current catalog snapshot with SELECT. If that snapshot
drifts or is missing, the split API login fails startup until the offline
migrator reconciles it.

Owned host PostgreSQL tests validate actual separate login identities, the exact
Python driver/SQL asset, fresh and populated pre-9 candidate installation, and
existing cache assertions. Compose smoke tests ran both profiles with a cached
API image and verified the executable entrypoint, UID, and separate read-only
configuration mounts. Full runtime acceptance requires a current API image and
the target deployment grants. Candidate 9 remains outside the ledger, and all
seven recorded-consumer gates remain false.

For the real Python-driver path, provision a task-owned psycopg-3 environment and
set `ATTUNE_TEST_CACHE_DBA_PYTHON` to its absolute interpreter path when running
the `recorded_rollout_tests` target. Follow the normal owned PostgreSQL setup and
unique run-ID instructions in [Running tests](../../docs/testing/running-tests.md),
with `--nocapture --test-threads=4`. Without that explicit interpreter, the Rust
fixture executes the same SQL asset directly. The host lane uses separate LOGIN
identities and trust authentication. For password and TLS coverage, supply an
isolated PostgreSQL instance with SCRAM and TLS required on TCP. The fixture
passes through the test database URL's password and `sslmode` to the Python
driver using its private credentials file.
