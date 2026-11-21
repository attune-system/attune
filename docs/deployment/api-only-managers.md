# API-only workload managers

The internal enrollment and session renewal boundary is available for integration
testing. The action worker and sensor binaries still use their existing
control-plane connections. Keep the existing manager deployment settings until
both clients support API-only operation.

## Enrollment contract

Configure `manager_enrollments` in the API configuration. Each entry has a
`role` of `action` or `sensor`, a pool name, and `secret_sha256` containing the
lowercase SHA-256 digest of the pool credential. Generate 32 random bytes for each
pool and encode them as 64 hexadecimal characters. Compute the digest over those
64 characters, excluding the final newline. Give the API replicas the same grant
list and an explicit deployment signing key.

```yaml
manager_enrollments:
  - role: action
    pool: build
    secret_sha256: "<64 lowercase hex characters>"
```

The manager submits its pool credential as the bearer at
`POST /api/v1/internal/managers/enroll`. The JSON body contains `role`, `pool`,
`name`, and `instance`. Pool and manager names have at most 63 characters. Start
with a lowercase ASCII letter or digit and use those characters or hyphens for
the rest of the name. An instance UUID is generated at process startup and reused
when retrying enrollment. An enrolled slot is exclusive while a session is live.
After expiry or withdrawal, a replacement uses a fresh instance UUID. Old
instance IDs stay retired.

The response contains the session ID, instance, expiry, and bearer. Send that
bearer to `POST /api/v1/internal/managers/session` to renew it before its
two-minute expiry and to `DELETE /api/v1/internal/managers/session` to withdraw
it. The API checks the credential and the persisted session using database time.
Renewal issues a new bearer. An earlier bearer stays valid until its original
expiry or session revocation. Withdrawal is safe to retry with an unexpired
bearer. Remove or rotate the
pool grant on all API replicas to revoke its sessions. During a rolling grant
change, replicas still running the old configuration can accept the old grant.
For credential rotation, withdraw the old instance before replacing its grant
or allow the exclusive slot's session to expire.

Run the session acceptance test against a disposable database using the
[host test setup](../testing/running-tests.md#runner-commands).

```bash
ATTUNE_TEST_RUN_ID=manager-sessions cargo test -p attune-api \
  --test manager_session_tests -- --test-threads=4
```

The test uses HTTP clients with pool credentials and API replicas sharing the
database. It exercises scope rejection, retry, expiry, and replacement.

## First-party chart and installer cutover

Once both API-only managers are ready, the chart and installer need to provision
one enrollment secret per role and pool. Mount the raw key only in the
corresponding managers and distribute its digest to every API replica. Use stable
manager names within a pool and new process instance IDs on restart. Persist the
worker's spool directory and dedicated custody key together if deployments need
log recovery after process replacement. Keep both out of child mounts and
passthrough environment. A StatefulSet ordinal can preserve a manager name and
its spool volume through Pod replacement.

The API retains PostgreSQL, RabbitMQ, signing, encryption, and object storage
access. Managers reach the API over HTTPS, including the existing archive,
artifact, log, and scoped sensor event routes. Deploy the trust roots for the
manager-facing HTTPS endpoint with the manager image or cluster trust bundle.
In RWO deployments, each manager uses its local cache and retrieves
version-pinned archive and prepared-runtime bytes through the API. Use the
existing Bubblewrap profile for local process execution.

Roll out the matching API and executor before enabling canary manager pools.
Exercise action placement, cancellation and retry, sensor generation handover,
candidate tests, runtime refresh and preparation, and log recovery through
completion. Drain the direct-connected pools after canary validation. At cutover,
remove database, broker, platform signing, master encryption, and object-store
credentials from manager deployments. Enforce the manager-to-API network path
while retaining the DNS and workload egress required by the deployment.
