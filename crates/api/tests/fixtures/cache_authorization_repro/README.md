# Managed-cache authorization reproduction

Synthetic fixture for the managed-cache declaration-reference/namespace mismatch.
It has no Salesforce, PagerDuty, workflow, sensor, rule or queue integration.
It includes a minimal pack-owned Python runtime definition, so publication does
not depend on the core pack already being installed.

The pack declares two snapshot caches owned by `cache_auth_repro`:

- `cache_auth_repro.users` has namespace `repro.users` and exposes the mismatch.
- `cache_auth_repro.control` uses the same declaration reference and namespace
  and serves as the successful control.

The writer grants only read/create/update for these two declaration references.
The platform supplies its normal managing-pack constraint during publication.

Run the real API reproduction with owned disposable dependencies:

```sh
bash scripts/run-rust-integration-tests.sh --test cache_authorization_tests -- --nocapture
```

Alternatively, provision disposable PostgreSQL as described in
[`docs/testing/running-tests.md`](../../../../../docs/testing/running-tests.md),
run `make db-test-setup` with that cluster's `TEST_DB_ADMIN_URL` and `TEST_DB_URL`,
and run from the repository root with a fresh run ID:

```sh
ATTUNE_TEST_RUN_ID=cache-repro-unique \
ATTUNE__DATABASE__URL="$TEST_DB_URL" \
cargo test --locked -p attune-api --test cache_authorization_tests -- --nocapture --test-threads=4
```

The tests explicitly approve only this fixture directory as a server-local source,
register the actual pack through the API, verify concrete admission
bindings and canonical runtime resolution, and start a delegated execution owned
by a separate sponsor assigned only the scoped writer. Cache calls use that
execution's token. Requests run through the real API router using the existing
delegated-execution fixture helper. Additional cases cover canonical reads and
listing without `standard`, publication through abandonment, name collisions,
owner/managing-pack/action constraints, retirement and tombstones, unmanaged/prospective targets,
recorded authority, and signed sensor authority. The sensor fixture admits and
mints a fenced workload credential without starting a sensor process.

On an affected platform, metadata GET succeeds, the control generation POST
returns 201, and the differing-name generation POST returns 403 with an audit
denial against `repro.users`. The test deliberately expects 201 for both writes,
so it fails until the platform defect is fixed. It prints sanitized response and
audit evidence and awaits fixture teardown even on the expected failure.

The `cache_auth_repro.probe` action provides the same generation POST
through the worker's `ProcessRuntime` in the automated suite. The test binds an
ephemeral loopback HTTP listener, runs the actual Python probe with a delegated
execution credential, and joins the server before database teardown. It consumes
JSON parameters on stdin and takes API credentials only from the execution environment. Each call
creates an empty staging generation if authorized; use only a disposable test
installation. This covers the real worker runtime and HTTP request; it does not
exercise executor dispatch or a separately running worker service.

## Operator deployment verification

The fix changes permission matching for existing managed caches; route names,
stored cache identities, generations, and permission actions remain the same.
This fix introduces no schema migration and requires no database reset or cache
recreation. Apply any other migrations required by the target platform release
and deploy its matching service versions through the normal upgrade procedure.

Before deployment, inspect authored cache grants for managed caches whose
declaration and namespace differ. Grants already selecting declaration refs stay
unchanged. Namespace-only grants for those managed caches must select the
declaration ref through normal permission publication. Keep the owner and
managing-pack constraints; do not add both names or broaden the writer grant.
Unmanaged namespaces continue to use namespace refs.

After an operator authorizes deployment of the tested platform revision:

1. Use a disposable installation to publish this fixture through the normal
   pack upload path. Give the test sponsor only the necessary action execution
   authority and this fixture's writer permission set.
2. Run `cache_auth_repro.probe` through the executor and a worker service with
   `namespace: repro.users`, then with `namespace: cache_auth_repro.control`.
   Both executions should exit successfully and report HTTP 201. They create
   empty staging generations; remove the owned fixture after verification.
3. On the original installation, the operator can separately authorize a fresh
   private workflow execution using its unchanged declaration-scoped writer set
   and fresh secret inputs. Verify refresh can upload, seal, and promote caches.
   This integration check requires no transfer of the private pack to this repo.

Deployment and the private workflow run are not performed by the automated tests.
