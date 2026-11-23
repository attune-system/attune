# API-only workload managers

Action and sensor managers can run through the Attune API with scoped
enrollment and sessions. Action managers execute actions and pack jobs.
Sensor managers run admitted sensor generations. Both keep local runtime
processes, Bubblewrap, and pinned pack and prepared-runtime caches. The API
performs authorized state changes and object transfers on their behalf.

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
    labels:
      site: internal
    taints: []
```

The manager submits its pool credential as the bearer at
`POST /api/v1/internal/managers/enroll`. The JSON body contains `role`, `pool`,
`name`, and `instance`. Pool and manager names have at most 63 characters. Start
with a lowercase ASCII letter or digit and use those characters or hyphens for
the rest of the name. An instance UUID is generated at process startup and reused
when retrying enrollment. An enrolled slot is exclusive while a session is live.
Set placement `labels` and `taints` on the API's pool grant. The API assigns the
reserved `managed_pool` label from the pool name.
After expiry or withdrawal, a replacement uses a fresh instance UUID. Old
instance IDs stay retired.

The response contains the session ID, instance, expiry, and bearer. Send that
bearer to `POST /api/v1/internal/managers/session` to renew it before its
two-minute expiry and to `DELETE /api/v1/internal/managers/session` to withdraw
it. The API checks the credential and the persisted session using database time.
Renewal issues a new bearer. An earlier bearer stays valid until its original
expiry or session revocation. Withdrawal is safe to retry with an unexpired
bearer. Remove or rotate the
pool grant on all API replicas to reject further manager requests. Withdraw each live session to revoke
its issued child and delivery authority immediately. During a rolling grant
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

## Action assignment control

The executor places an execution and records its offer in one PostgreSQL
transaction when the chosen worker has a server-owned manager binding. Ordinary
workers continue to receive broker dispatches. Enrollment alone never creates a
worker binding or changes placement.

The internal client uses these operations under `/api/v1/internal/managers/work`.

| Operation | Contract |
| --- | --- |
| `GET /` | Wait up to eight seconds for an outstanding offer. PostgreSQL notifications wake the waiting API replica, which rereads durable state. |
| `POST /{assignment}/claim` | Start the exact scheduled execution for this registration and release. Retrying the same request UUID gets the same live claim. |
| `POST /{assignment}/lease` | Extend a running assignment for 30 seconds or return a cancellation instruction from persisted execution state. `?wait=true` waits up to eight seconds and wakes when a cancellation commits. |
| `POST /drain` | Stop accepting new offers and retain authority for already claimed assignments. |
| `POST /{assignment}/finish` | Classify the result using the stored output contract, settle the execution, and record completion for publication in a single transaction. A matching retry returns the committed status. |

The claim response pins the assignment and release. The same UUID authorizes
`POST /{assignment}/launch`, which returns the admitted inputs and runtime
selection. The API decrypts inputs after validating their provenance. It issues
an execution API token only for an opt-in permission selection. The action
receives its JSON parameters through stdin. Native and command launch metadata
retain their declared nonsecret CLI bindings and record the selected program
against the same assignment.

An expired unclaimed offer returns to executor scheduling with its original
request attribution. An expired claimed attempt settles as abandoned, or
cancelled if cancellation is already pending. The API republishes completion
with its stable event ID until the executor records a durable receipt. The
executor serializes concurrent deliveries. A crash during processing can replay
the event through the existing completion and workflow repository fences.

For installations with separate PostgreSQL roles, the API and executor need
access to the manager session, assignment, and completion tables for their
respective operations. The supervisor reads the assignment table when deciding
whether it owns stale execution recovery. An execution cancellation uses the
writer's access to read the assignment and notify its manager session.

For protected object logs, the prototype exercised the existing v1 frames and
final manifest with a random, API-wrapped key per stream. The manager would
receive the stream key during the live attempt and spool the resulting exact
ciphertext under its local custody key. A replacement can then resume delivery
with its own session and retained custody, while the API unwraps the stream key
when verifying or reading the stored log. The live transport still needs to
wire key issuance, wrapped key storage, and authorized custody resumption.

To exercise broker return and redelivery, use an exclusively owned disposable
RabbitMQ vhost and the same disposable database setup as the session tests.
Set `ATTUNE_MANAGER_TEST_AMQP_URL` to the vhost's URL and run the external test.

```bash
ATTUNE_TEST_RUN_ID=manager-outbox cargo test -p attune-api \
  --test manager_session_tests completion_survives_broker_return \
  -- --ignored --test-threads=4
```

The test also starts the executor service, places an admitted execution through
the ordinary request queue, and waits for the executor to record its completion
receipt.

## Launch and handover prototype results

The launch prototype restored the bound input contract into stdin and preserved
the selected delegated owner or recorded sponsor in the existing execution
credential. Retrying the same claim preserved the attempt. A tokenless action
could fetch its pinned release through scoped manager authorization. The test
exercised versioned S3 with the object key and provider version resolved by the
API.

Production launch needs to bind the child credential's consumers to the same
live manager assignment. The existing action validators still accepted the
attempt after its manager lease expired, until executor recovery. The existing
sensor event route similarly accepted the old generation after manager
withdrawal, until its workload fence closed. Require the live manager session
and the exact action attempt or sensor generation in the transaction that owns
each protected effect. Recheck the deadline after any lock or I/O wait before
committing an effect or handing off protected data. Immediate withdrawal
requires checking persisted session state.

Sensor handover can reuse the existing workload assignment and generation
fences. The receipt prototype committed generation advance, schema admission,
and the request receipt together. Concurrent duplicates and a retry returned
one pinned generation. Rollback restored the prior generation and removed the
uncommitted admission and receipt. Withdrawal or replacement invalidated
replay. Store identifiers in the receipt and resolve the admitted inputs
through the same live fence on replay.

The log resumption prototype reopened Attune's encrypted spool under a new
process incarnation. The replacement used the same logical worker slot and
exact custody identifier to obtain a narrow delivery credential, then uploaded
and sealed the exact bytes through the existing routes on versioned S3. The
API unwrapped a distinct key for the log version. After sealing committed, a
retry received a ready disposition even when the delivery intent still showed
active. A ready reply gives the holder evidence to remove its local spool.

## Action and sensor managers

Run `attune-worker --api-manager` or `attune-agent --api-manager` for actions
and pack jobs. Run `attune-sensor --api-manager` or
`attune-sensor-agent --api-manager` for sensors. All four entrypoints read their
local manager settings before the ordinary service configuration.

The action manager claims bounded concurrent work from a scoped HTTP queue. The executor still
places actions, handles retries, and runs workflows. Shutdown drains incoming
work and allows bounded completion of active executions and encrypted custody.

The sensor manager reconciles eligible workloads through the API. Startup
commits the generation, pinned schema, restored bootstrap authority, and
idempotency receipt together. The scoped sensor token can ingest only admitted
trigger events. Lease renewal rechecks placement, demand, and changes to the
sensor definition or installed release. The manager stops the process and joins
its writers before releasing its generation. A replacement gets a higher
generation. A stale process cannot ingest events or write the replacement's
logs. Sensor log rotation and provenance use existing private sensor artifacts.

For protected action logs, the API allocates the exact version and derives an
independent key from the platform encryption key and the version's server-owned
binding. Managers receive only that version key. Live append uses a
version-scoped credential that rechecks the current manager and assignment
lease. Delegated actions persist the encrypted spool before submitting their
result. Once that result commits, the current holder of the original slot can
resume exact bytes using the persisted custody ID. The API verifies the existing
frame and manifest contract before publishing Ready. Existing readers can
continue to authorize and decode both kinds of encrypted logs. Recorded
deliveries retain their existing approval and delivery-permit checks.
Recorded scheduling still requires the
[coordinated consumer rollout](../architecture/recorded-consumer-readiness.md).

## Runtime placement and pack jobs

Managers fetch the runtime catalog and probe locally with the existing
interpreter checks and Bubblewrap recipe discovery. An operator must admit
recipe digests in the manager environment before those probes run. An explicit
runtime filter still narrows discovery. API reports carry a catalog revision
and per-instance sequence. The API resolves runtime identities and registered
versions, projects the existing placement capabilities, and returns a receipt.
Identical retries return the receipt. Stale revisions or sequences, or changes
to an accepted sequence, fail.

Changes to probe definitions retire the prior reports for placement without
interrupting live attempts. A manager stays ineligible until it reports against
the current catalog. The same slot keeps its worker ID across process
replacement. For execution and pack offers, placement checks the selected
worker again in the transaction that persists the offer.

When PostgreSQL roles are separated, catalog writers need update access to
the catalog revision row for the runtime triggers. The API updates reports
and pack jobs. The executor reads reports and manages pack offers and recovery.
The supervisor reads active retest pins during release reclamation.

The executor places pack tests and preparation using runtime capabilities,
operator labels, taints, placement rules, and the eligible preparation scope.
For an API manager, it records one durable HTTP offer and leaves the broker
dispatch queue unused for that job. An offer can wait behind ongoing work
within the install budget. The short lease starts at claim, and a claim retry
keeps the same attempt.
Optional candidate effects use the existing isolated effects route. The
credential carries authority through the exact manager session and job lease
and reaches only opt-in test children. The authenticated completion route
accepts up to 100 MiB of test results, including captured runner output.

The API serves version-pinned candidate or release archives through the
existing streaming routes. Prepared runtime upload and completion use one
claim, exact request evidence, and the install's own deadline. For object
storage, the API stages bytes before taking the publication transaction. It
rechecks the current session and claim when attaching the object and records
the receipt in the same transaction. An upload authorized before bearer expiry
can finish under the same still-live session and job lease. An identical
completion replay is safe after activation.

The executor recovers abandoned offers and expired or withdrawn claims within
the existing install budget, with up to three offered attempts. A replacement
receives a fresh claim. Active-release retests retain their original release
pin. Prepared-download access requires the exact running action attempt or
admitted sensor generation and its release pin and component. In RWO
deployments, the consumer restores the pinned archive into its local cache at
the configured runtime path.

## Configure the manager processes

`--api-pack-jobs` remains available on the worker binaries for a pack-only
canary. Its registration is excluded from execution placement.

| Setting | Value |
| --- | --- |
| `ATTUNE_API_URL` | HTTPS URL for the Attune API. Install its trust roots in the image or platform trust bundle. |
| `ATTUNE_MANAGER_POOL` | An API enrollment grant for the process's `action` or `sensor` role. |
| `ATTUNE_MANAGER_NAME` | Stable process slot name within the pool. Worker and universal agent binaries also accept `--name`. |
| `ATTUNE_MANAGER_ENROLLMENT_FILE` | Read-only pool secret file mounted outside workload-visible directories. |
| `ATTUNE_WORKLOAD_SANDBOX` | `bubblewrap`. |
| `ATTUNE_PACKS_BASE_DIR` | Writable local cache for pinned releases. |
| `ATTUNE_RUNTIME_ENVS_DIR` | Writable preparation root at the same absolute path across compatible producers and consumers. |
| `ATTUNE_ARTIFACTS_DIR` | Private local artifact scratch root for action or sensor managers. |
| `ATTUNE_MANAGER_ACTION_CONCURRENCY` | Maximum concurrent actions in one action manager, 1 to 32. The default is 10. |
| `ATTUNE_MANAGER_LOG_SPOOL_DIR` | Private persistent custody directory for each `--api-manager` action slot. |
| `ATTUNE_MANAGER_LOG_SPOOL_KEY_FILE` | Stable manager-only file containing 64 hexadecimal characters for that spool. |
| `ATTUNE_RUNTIME_RECIPE_DIGESTS` | Operator-admitted recipe digests for lifecycle preparation and test runners. |
| `ATTUNE_RUNTIME_PREPARATION_REVISION` | Shared nonsecret toolchain revision when using prepared runtimes. |
| `ATTUNE_WORKER_RUNTIMES` | Optional action runtime name filter. |
| `ATTUNE_NOTIFIER_WS_URL` | Optional scoped child notifier URL for sensors that subscribe to rule changes. |

Manager action channels use the API worker byte limits, capped at 16 MiB per
channel for bounded local custody. Silent streams allocate no log versions.
The API owns object storage access for release and runtime archives and log
segments. Children use the existing scoped artifact APIs for file publication.

Use disposable PostgreSQL for session, placement, and sensor-generation checks.
With the versioned S3 fixture and Bubblewrap, run both services through scoped
HTTP and verify replacement custody through the production routes.

```bash
ATTUNE_TEST_RUN_ID=manager-workloads RUST_MIN_STACK=33554432 cargo test -p attune-api \
  --test manager_session_tests workloads -- --include-ignored --test-threads=4
```

 The pack-job
acceptance test additionally requires an owned RabbitMQ vhost, versioned object
storage, and Bubblewrap. Set `ATTUNE_MANAGER_TEST_AMQP_URL` and the S3 fixture
variables from the [runtime log correctness setup](runtime-log-verification.md#run-the-correctness-suite).

```bash
ATTUNE_TEST_RUN_ID=manager-pack-jobs RUST_MIN_STACK=33554432 cargo test -p attune-api \
  --test manager_session_tests manager_pack_jobs_recover_and_stream \
  -- --ignored --test-threads=4
```

## First-party chart and installer cutover

For each API-only pool, the chart and installer need to provision one enrollment
secret per role and pool. Add action and sensor canaries alongside direct-connected pools. Mount the raw
key only in the corresponding managers and distribute its digest to every API
replica. Use stable manager names within a pool and new process instance IDs
on restart. Persist the
worker's spool directory and dedicated custody key together if deployments need
log recovery after process replacement. Keep both out of child mounts and
passthrough environment. A StatefulSet ordinal can preserve a manager name and
its spool volume through Pod replacement.

The API retains PostgreSQL, RabbitMQ, signing, encryption, and object storage
access. Keep the login and app configuration volumes out of the manager
containers. Mount only the enrollment and action custody secret files plus the
manager's runtime cache and artifact roots. Managers reach the API over HTTPS, including the existing archive,
artifact, log, and scoped sensor event routes. Allow the eight-second manager
long polls and streaming archive transfers through the ingress. Deploy the trust roots for the
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
