# Pack candidates across API replicas

Pre-install pack tests use the same configured immutable object backend as
installed pack releases. Candidate downloads do not read API-local staging
directories, even when the worker uses `packs.transport: volume` for installed
releases.

## Deployment requirements

- All API replicas must share PostgreSQL and the same `storage` bucket and
  prefix. S3 requires bucket versioning; GCS is also supported. A filesystem
  backend is suitable only when its storage root is shared across replicas.
- Configure workers with `ATTUNE_API_URL` pointing at this deployment and
  worker-token authentication. Workers need no object-provider credentials.
- Set `packs.transport: api` on workers without a shared installed-release
  volume. Shared-volume workers still download candidates through the API.

The API replica accepting an install validates the candidate, builds a bounded
archive into a temporary file, and uploads it before publishing the test
request. The install record pins the object key, provider version, size, and
SHA-256 digest. Upload or database publication failure prevents test dispatch
and activation; it does not bypass tests.

Any API replica can stream the pinned object to the assigned worker. Access
requires both that worker's JWT and the attempt-specific candidate token, and
is denied for expired or completed attempts. Workers verify the digest before
extracting all pack components into `.pack-test-attempts/<install-id>/<pack-ref>`.
The active release and API staging tree are not modified by worker cleanup.

Candidate objects remain protected from object maintenance during pending
dispatch, running tests, and activation, subject to the existing lifecycle
deadlines (seven hours for tests, fifteen minutes for activation). Failed,
completed, or expired attempts release that protection. The existing object
maintenance ledger owns eventual exact-version deletion, including uploads
abandoned before database publication.

## Optional isolated API effects

`testing.candidate_api_effects: true` requests a separate bounded runtime context
after archive verification. It does not turn the archive-download credential
into an execution token. Candidate Keys, triggers and events use isolated tables
and never enter production rules or notifications. Finishing tests revokes the
context before activation. Deployment must verify closed-context evidence for
opt-in attempts, including results returned by older workers. See
[candidate API effects and bootstrap admission](candidate-api-effects.md).

## Rollout

Apply `20261012000001_pack_install_candidate_objects.sql` and roll out updated
API, worker/agent, and supervisor binaries together. The supervisor must use
the new candidate retention pins. Previously staged, unfinished attempts have no durable
candidate pointer and must be retried after rollout. There is deliberately no
fallback to a replica-local candidate archive. No database reset or test bypass
is required.

## Regression tests

`pack_install_candidate_tests` covers write-once publication, lifecycle pins,
deletion rechecks, and expiry. The ordinary `pack_registry_tests` candidate
tests cover publication before dispatch, rollback on dispatch failure, scoped
downloads from a replica with a separate staging root, and extraction in both
worker transport modes.

The additional `candidate_archive_distributes_components_across_s3_api_replicas`
test uses separate S3 clients with a unique test-owned prefix. It requires the
versioned S3 harness and environment described in
[Runtime log verification](runtime-log-verification.md#run-the-correctness-suite).
Run it explicitly after harness setup:

```bash
ATTUNE_TEST_RUN_ID=pack-candidate-s3 cargo test -p attune-api \
  --test pack_registry_tests \
  candidate_archive_distributes_components_across_s3_api_replicas \
  -- --ignored --exact --test-threads=4
```

The external-service test is excluded from infrastructure-free runs only; pack
installation tests are not skipped by this change.
