# Data Caches

## Status

The required initial cache subsystem is implemented in the current worktree.
This document records its design, contracts, and validation scope. Optional
refresh sets, single-response NDJSON scans, and SQLite exports remain future
extensions.

## Decision

Do not use `key` records as a record cache. Keep them for small configuration
values and secrets. Add a dedicated, owner-scoped cache model for queryable
external records such as Salesforce Users.

A **Data Cache** is an Attune-local, reconstructable automation snapshot of
data whose authoritative copy lives elsewhere. It exists so actions, workflow
tasks, and sensors can repeatedly consume a coherent published snapshot
without overloading or depending on the latency of the source system. A cache
must be rebuildable from that source or another durable, versioned export.

Data Caches are not:

- authoritative business-data storage or a system of record;
- a general query database, data warehouse, search engine, or reporting store;
- a place for credentials, tokens, encryption keys, or other secrets.

Keep authoritative writes, durable business history, cross-dataset joins,
ad-hoc filtering, analytics, and regulatory retention in an independent system
designed for those responsibilities. Keep secrets in Keys and Secrets. Losing
all cache generations must be recoverable by refreshing them; if it is not,
the data does not belong in a Data Cache.

An artifact containing SQLite can be a useful immutable snapshot distribution
format, but it is not a replacement for a shared, queryable cache API.

Actions, sensors, operators (CLI and web), and external SDKs all consume the
same authenticated HTTP API documented by OpenAPI. Language clients should be
generated from that contract; Attune intentionally has no bespoke in-tree Rust
cache SDK.

## Design Intent and Invariants

The cache exists to make high-cardinality external data available to Attune
workloads without repeatedly calling a slow upstream system. It is not a
general replacement for Keys and Secrets, artifacts, or a relational
integration database.

The design must preserve these invariants:

- **Deliberate access:** cache data is fetched only by a caller that needs it;
  it is never ambiently injected into every action in a scope.
- **Scoped isolation:** an owner scope and namespace identify the data boundary
  for authorization, lifecycle, and queries. Different datasets in one scope
  remain independent by default.
- **Canonical ownership:** API-facing owner refs are resolved to canonical
  owner IDs before storage and authorization. Renames or stale denormalized
  refs must not create a second logical owner or bypass an owner constraint.
- **Coherent reads:** a workload can enumerate all entries from one published
  snapshot even while a newer refresh is being prepared or promoted.
- **Atomic publication:** a failed or incomplete refresh is never visible, and
  a slow writer cannot replace a newer completed refresh accidentally.
- **Least privilege:** cache data can contain sensitive business information,
  but it is not a credential store. Access is explicit, scoped, auditable, and
  separate from secret decryption.
- **Bounded operation:** 200,000-record reads and writes must stream in
  bounded memory and request sizes, while retention eventually reclaims old
  data without breaking active readers.
- **Bounded storage:** per-record, per-generation, per-namespace, staging, and
  aggregate owner/deployment admission limits prevent one or many integrations
  from consuming unbounded PostgreSQL storage before retention runs.
- **Reproducibility:** optional offline exports identify the cache generations
  from which they were built.

## Why the Existing Key Store Is Not Suitable

The current `key` table has the right properties for configuration and secrets:
JSON values, optional encryption, ownership, RBAC, and exact lookup by a
globally unique `ref`. It does not have the data model or access patterns
needed for a 200,000-record cache.

| Requirement | Current key behavior | Result |
| --- | --- | --- |
| Scope + namespace + external ID lookup | Keys have a global `ref` and ownership fields, but no namespace or external ID. | A compound record identity cannot be queried or indexed directly. |
| Enumerate cached values | `GET /keys` supports owner filters and offset pagination, but intentionally redacts values. | A full read requires list requests plus an individual read for every record. |
| Stable full traversal | The list endpoint uses independent count and offset queries. | Concurrent writes can shift pages, producing duplicates or omissions. |
| Bulk ingestion | Keys are created and updated one at a time, without batch upsert, staging, or promotion. | A 200,000-record refresh is slow, expensive to audit, and exposes partial state. |
| Cache lifecycle | Keys have no expiration, source revision, generation, or freshness state. | Staleness and a coherent "all records as of time T" read cannot be represented. |
| Action delivery | The worker loads all system, pack, and action keys and passes them through the stdin secret channel. | A large pack-scoped cache would be delivered to every action in that pack. |

Encoding a cache namespace and external ID into `key.ref` does not fix these
problems. It keeps the global-reference constraint, provides no supported
namespace scan, and retains the ambient secret-injection behavior.

## Recommended Data Model

Model the cache as an owner-scoped collection with immutable generations.
Use the same owner semantics as keys so a cache can be scoped to a pack,
action, sensor, identity, or the system. The cache's logical scope is its
owner, not an arbitrary free-form string.

```text
cache_namespace
  id BIGINT
  owner_type + exactly one canonical owner ID (or none for system)
  denormalized owner refs for display/token matching
  optional definition_ref + managing_pack/managing_pack_ref provenance
  namespace
  active_generation
  freshness_target_seconds
  max_records_per_generation, max_generation_bytes
  max_retained_bytes, max_retained_generations
  tombstoned_at, tombstone_reason
  created, updated

cache_generation
  id BIGINT
  namespace_id
  state: staging | ready | active | retired | failed
  client_refresh_id
  expected_chunk_count, expected_count, expected_bytes
  record_count, size_bytes
  checksum_algorithm, checksum
  source_revision
  created_by
  created, sealed, activated, retired, readable_until

cache_entry
  id BIGINT
  generation_id
  external_id TEXT with deterministic bytewise collation
  value JSONB
  source_updated_at
  source_checksum
  size_bytes
  created

cache_ingest_chunk
  id BIGINT
  generation_id
  chunk_index
  request_checksum
  record_count, size_bytes
  created
```

`ready` is a sealed, validated generation: entry writes are allowed only while
the generation is `staging`, and promotion is allowed only from `ready`.
Without this intermediate state, records could be appended after validation
but before promotion.

At most one live namespace may exist within a canonical owner scope. The live
unique index excludes tombstoned rows so a declarative pack definition can be
reinstalled while its predecessor drains. The canonical owner trigger still
enforces exactly one matching live owner ID. Owner refs and the canonical
owner text are durable snapshots, not the live uniqueness authority.

A cache entry must be unique on `(generation_id, external_id)`. External IDs
are non-empty, opaque, and case-sensitive; do not lowercase them. Define a
maximum encoded length and use a deterministic `C`/bytewise collation in both
the uniqueness index and cursor queries so ordering does not change with
database locale.
This supports:

- Exact lookup by owner scope, namespace, and external ID.
- Cursor scans ordered by `external_id`.
- Immutable reads against a specific generation.

The required indexes are:

- A unique namespace index on the canonical owner scope and `namespace`.
- A unique bytewise index on `(generation_id, external_id)`.
- A partial unique index on `namespace_id` for rows whose state is `active`.
- A unique `(namespace_id, client_refresh_id)` index for idempotent refresh
  creation.
- Generation lifecycle indexes on `(namespace_id, state, created)` and
  `(state, readable_until)`.
- A cleanup index on `(generation_id, id)` for bounded entry deletion.
- A unique `(generation_id, chunk_index)` index for idempotent chunk replay.

All cache entities use `BIGINT` IDs. Foreign keys from namespace ownership may
target normal identity, pack, action, and sensor tables. Pack/action/sensor
deletion first tombstones affected namespaces in the same lifecycle operation,
then clears typed owner FKs while retaining canonical owner text and refs for
audit. Generation and entry rows are never cascaded or synchronously deleted.
The active-generation relationship must also verify that the selected
generation belongs to the same namespace. Implement that with a composite
foreign key/constraint, such as a unique generation `(id, namespace_id)` pair
referenced by namespace `(active_generation, id)`, or enforce it in the locked
promotion transaction; a plain foreign key on `active_generation` is
insufficient.

`active_generation` is the read source of truth. The redundant generation
state is maintained in the same promotion transaction, with at most one
`active` generation per namespace, so operational queries cannot observe a
pointer/state contradiction.

Do not index arbitrary fields inside `value` until a concrete query pattern
requires them. The initial API should support point lookup, bounded
multi-ID lookup, and complete namespace enumeration only.

### Declarative Pack Deployment

Packs may declare namespaces in `caches/*.yaml`. Each flat definition has a
stable pack-qualified `ref`, immutable `namespace`, `owner_type`, and
`owner_ref`, plus mutable policy fields. Pack definitions support only
pack-, action-, and sensor-owned namespaces, and action/sensor refs must belong
to the installing pack.

Pack-managed rows store `definition_ref`, the current managing pack ID, and a
durable managing pack ref. This provenance is internal and deliberately
separates declarative definitions from API-created namespaces:

- pack reload cleanup tombstones only live definitions managed by that pack;
- unrelated API-created namespaces survive ordinary definition removal;
- policy updates preserve the namespace ID and active generations;
- definition owner or namespace changes are rejected;
- removing an owning action or sensor tombstones all namespaces using that
  owner, including API-created ones, before deleting the component;
- pack deletion tombstones all affected namespaces transactionally before
  deleting the pack.

Pack-managed namespaces are immutable through the namespace update and delete
APIs. Change or remove their `caches/*.yaml` definition instead; API refresh and
read operations remain available subject to authorization.

Tombstoned definitions are immediately absent from normal reads. The
supervisor continues to drain their generations and entries by namespace ID.
Because live uniqueness excludes tombstones, reinstalling the same definition
before drain completion creates a new live namespace ID and cannot resurrect
or attach to the old generation chain. API namespace creation retains its
existing conflict behavior while any row still occupies that owner/name slot.

## Snapshot and Pagination Semantics

Offset pagination is not sufficient for a refreshing cache. The first list
request should resolve the namespace's active generation and return that
generation identifier with a cursor:

```text
GET /cache/namespaces/salesforce.users/entries
    ?owner_type=pack&owner_ref=salesforce
  -> generation: 12345
  -> items: [...]
  -> next_cursor: "..."
  -> cursor_expires_at: "..."
```

Every later page must include the generation identifier and cursor. The server
must read that immutable generation even if a newer generation becomes active.
This guarantees that a job can retrieve every record that existed in one
published cache snapshot.

The cursor must be opaque, versioned, and integrity-protected or fully
revalidated as untrusted input. It carries at least the namespace ID,
generation ID, last external ID, page-shape version, and expiration. The query
is keyset pagination:

```sql
WHERE generation_id = $1 AND external_id > $2
ORDER BY external_id COLLATE "C"
LIMIT $3
```

The external-ID column/index collation and query collation must match. A cursor
must not be accepted for another namespace, owner, page shape, or caller-visible
generation.

Promotion sets the prior generation's `readable_until` to at least
`retired_at + maximum_traversal_duration`. Cleanup never removes an active
generation and never removes a retired generation before `readable_until`.
Because the retention clock starts at retirement, a traversal that began while
the generation was active receives the full advertised completion window after
promotion. Cursors expire at or before that boundary. A request for an expired
or removed generation returns a specific snapshot-expired error, not a
fallback to latest.

Freshness and retention are different. An active generation may become stale
when its freshness target is exceeded, but it remains readable with
`stale: true` until replaced or explicitly deleted. Callers may request
`require_fresh=true`; the default must not make the last known-good dataset
disappear merely because an upstream refresh failed. A
`freshness_target_seconds` value of `0` disables stale classification and
freshness alerts for that namespace.

A namespace with no active generation is uninitialized, not an empty dataset.
Return an explicit `cache_not_populated` result. A successfully published
zero-record generation is how a producer represents an authoritative empty
dataset.

Authorization is re-evaluated on every page. Permission revocation or token
expiration may stop a traversal even while its generation remains retained;
snapshot consistency is not an authorization lease. Cursor expiration should
not exceed the earlier of `readable_until`, the configured traversal limit,
and the current token expiration.

## Native Workflow Iteration

Workflow tasks consume complete cache snapshots with `iterate_cache`. The
executor scans the cache and schedules action executions without requiring an
HTTP pagination loop or loading the namespace into workflow context.

```yaml
- name: process_users
  action: salesforce.process_user_batch
  iterate_cache:
    owner_type: pack
    owner_ref: salesforce
    namespace: users
    generation: active
    require_fresh: false
    page_size: 100
  batch_size: 25
  concurrency: 4
  permission_set_refs:
    - standard
  input:
    users: "{{ item }}"
    batch_index: "{{ index }}"
```

The iterator accepts `owner_type`, owner-dependent `owner_ref`, `namespace`,
`generation`, `require_fresh`, and `page_size`. `owner_type` defaults to `pack`,
and a pack `owner_ref` defaults to the containing workflow's pack. `generation`
defaults to `active` and may instead be an integer generation ID or a template
resolving to either form. The generation is selected once. `require_fresh` defaults to
`false`, so stale last-known-good data remains usable unless the author rejects
it. `page_size` defaults to `100`, is bounded from `1` through `1000`, and is
only the executor's internal scan fetch size. It does not define a child input.

Task `batch_size` is supported independently, defaults to `1`, and is bounded
from `1` through `1000`. At `batch_size: 1`, `item` is one cache entry object:

```json
{
  "external_id": "user-001",
  "value": {"enabled": true},
  "source_updated_at": null,
  "source_checksum": null,
  "size_bytes": 48
}
```

At `batch_size` greater than `1`, `item` is an array of those entry objects; the
last array may be smaller. `index` is the stable zero-based batch ordinal, not
an internal page number. Entries retain bytewise external-ID order. Task
`concurrency` limits in-flight batches and defaults to `1`. A published empty
generation succeeds without running the called action; an unpopulated namespace
fails explicitly.

### Generation and Retention Pin

The first read pins one immutable generation. Every page and every child retry
uses that generation, even after a newer generation is promoted. An explicit
An explicit generation ID selected through `generation` must belong to the
namespace and be active or still-readable retired data.

The durable iteration row is the retention pin while its state is `scanning`.
Cache cleanup excludes its generation; there are no renewable leases, expiry
times, or lease fields. Normal workflow completion, failure, and cancellation
make the iteration terminal. Workflow terminal-state handling and supervisor
workflow remediation also terminalize abandoned scanning iterations, ensuring
that a terminal workflow cannot pin a generation indefinitely.

The executor persists the generation, last external ID, next batch ordinal,
counts, and child lineage. Restart and workflow resume continue from that
checkpoint and reconcile existing children rather than changing to the latest
generation or mixing snapshots.

### Permissions, Retries, and Failure

Native reads use the task's resolved `permission_set_refs`. The reserved
`standard` grant is read-only and limited to cache namespaces in the signed
executing/containing action and pack scopes. A different owner requires a
named permission set constrained to that owner and namespace. Every named ref
used by the task must already have been delegated onto the parent workflow
execution; a workflow cannot introduce a named ref while dispatching a child.
The iterator never receives cache write authority from `standard`, and the
called action needs a token only if it makes additional Attune API calls.

Task retries apply per child batch. A retry receives the same persisted child
input and batch ordinal. When a child exhausts retries, the iterator stops
discovering new batches, lets in-flight batches settle, and takes the normal
failed transition. Authorization denial, freshness rejection, an unpopulated
cache, an invalid explicit generation, and scan errors fail the iterator;
records are never silently skipped. The iterator does not maintain page-level
retry counters or page-level retry summaries.

Cache-iteration children are normal durable executions. If the transaction
creates a child in `requested` state but its MQ request is lost, supervisor's
requested-execution recovery republishes it after the configured grace period.
The idempotent workflow task identity prevents recovery from creating a second
child for the same batch ordinal.

### Privacy and Observability

`iterate_cache` is deliberate disclosure to the named child action, not ambient
cache injection. Entry values occur only in child inputs explicitly rendered
from `item`. The executor does not copy entries or external IDs into
workflow variables, parent/iterator results, audit events, transition events,
error messages, or structured iterator logs. Pack authors must not publish,
log, or return batch content unless that additional disclosure is intended.

The protected
`GET /api/v1/executions/{id}/workflow-cache-iterations` endpoint returns safe
status fields only: `task_name`, `namespace_id`, `generation_id`, `state`,
`scanned_count`, `dispatched_count`, `page_size`, `batch_size`, `concurrency`,
`created`, `updated`, `completed_at`, and a bounded `error_summary`. It omits the
cursor, entries, external IDs, and cache values. The workflow execution details
panel renders the task name, state, generation ID, scanned/dispatched counts,
batch size, page size, concurrency, timestamps, and bounded error summary. It
does not add lease, freshness, retry-summary, or failed-page fields.

Use `with_items` for a JSON array already present in workflow context. Use
`iterate_cache` for a potentially high-cardinality cache: it pages lazily,
holds a terminal-state retention pin, persists traversal progress, and does not
materialize the full dataset in workflow state.

## Bulk Refresh Protocol

Refreshes should be copy-on-write:

1. Create a `staging` generation, optionally with expected count and source
   revision, plus a client refresh ID and expected chunk count for idempotent
   create retries and completeness validation.
2. Stream entries in bounded, numbered chunks, preferably NDJSON with optional
   content encoding. Each chunk is atomic and records its request checksum so
   an identical retry succeeds without duplicating rows, while a different
   payload for the same chunk number fails.
3. Seal the generation: lock it, reject further entry writes, compute
   authoritative record/byte counts, require a contiguous accepted chunk set,
   validate duplicate external IDs, expected limits, and any supported
   checksum, then transition it to `ready`.
4. Atomically promote the ready generation by changing
   `cache_namespace.active_generation`.
5. Retire and later delete old generations according to retention policy.

Only the promotion changes what readers see. A failed or abandoned refresh
never becomes visible. Each transition from `staging` or `ready` to `failed`
increments a persisted namespace failure streak exactly once; retrying the same
failure is idempotent. A successful promotion resets the streak and its last
failure timestamp. The supervisor evaluates this persisted state, so restarts
and cleanup of failed generation rows do not erase repeated-failure history.

Concurrent publishers need a required optimistic expected-active-generation
value, with explicit `null` for the first publication, or a scoped refresh
lease. This prevents an older sync that finishes late from overwriting a newer
active generation. Any administrative force promotion should be a separate,
strongly authorized, prominently audited operation.

The initial refresh contract is full-snapshot replacement, not in-place
upsert of the active generation. If an upstream later provides efficient
deltas, an optional repository operation may clone the active generation into
a new staging generation with `INSERT ... SELECT` and then apply bounded delta
chunks. The clone remains quota-controlled and invisible until sealed and
promoted.

The ingestion API must enforce hard limits before and during upload: maximum
external-ID bytes, maximum JSON value bytes, records and bytes per chunk,
records and bytes per generation, concurrent staging generations per
namespace, and retained bytes per namespace/scope. `size_bytes` should use one
documented accounting rule, such as PostgreSQL `pg_column_size(value)` plus
identifier bytes, and completion must recheck the authoritative total.

Checksums are useful only when their bytes are well-defined. Store an algorithm
and format version. The initial release may omit whole-generation checksums and
rely on expected count/bytes plus per-chunk request digests; it must not claim
content equivalence from an unspecified JSON serialization. A later canonical
checksum can hash records in bytewise external-ID order using a documented
canonical JSON encoding.

Bulk endpoints should audit the operation as a summary: namespace, generation,
record count, source revision, outcome, and actor. They must never emit every
record or its value into audit data.

## Authorization and Execution Access

Add a distinct cache resource to RBAC rather than extending `Resource::Keys`.
Keys and caches have different value-disclosure, retention, and throughput
requirements.

- Cache reads should be scoped to the cache owner and namespace.
- Cache writes and generation promotion should require an explicit writer
  grant, constrained to the intended owner/namespace.
- The reserved `standard` execution access should grant read-only cache access
  for the executing action and pack, plus the containing workflow action/pack
  when those refs are included in the execution token's standard-access
  context. It must not grant writes.
- Execution cache access exists only when `permission_set_refs` is non-empty.
  Actions that need the cache must request `standard` or a named permission
  set; otherwise the worker intentionally omits `ATTUNE_API_TOKEN`.
- Managed sensor tokens carry the exact registered sensor ref, pack ref, and
  explicit cache permission-set/access snapshot used by cache routes. The
  `standard` grant is read-only for that sensor and pack; refresh/write access
  must be separately configured. The sensor manager automatically renews the
  token before expiry and performs a controlled process restart, retrying
  renewal while the sensor remains eligible and stopping it if renewal cannot
  complete before expiry.
- Worker and refresh tokens should be rejected from cache data routes unless a
  narrowly defined internal operation is added later.
- Values that are credentials or other secrets remain in Keys and Secrets,
  not in cache entries.

Actions, workflow tasks, and sensors should fetch cache entries deliberately
through the cache HTTP API, preferably with a generated SDK. Cache data must
never be added to ambient action parameters or the secret stdin channel.

All cache routes are protected with `RequireAuth`, but authentication alone is
not authorization. Access/execution tokens use effective cache grants; sensor
tokens use only their signed sensor cache authority; other token types fail
closed. List, lookup, count, stream, and generation metadata queries must apply
the same owner/namespace visibility predicate in the repository query so
counts and existence checks do not leak inaccessible namespaces.

External IDs may themselves be customer identifiers. Prefer point and multi-ID
lookup bodies over putting IDs in URLs or query strings that are commonly
logged. Audit and request logs record namespace, generation, counts, outcome,
and actor, not payload values or raw external-ID lists.

The proposed `JSONB` payload is plaintext at the application/database layer and
will appear in PostgreSQL storage, WAL, replicas, and backups unless those
layers are encrypted. TLS, encrypted volumes/backups, log redaction, and data
classification are therefore required deployment controls. If a namespace
requires application-layer encryption, add it as an explicit later feature
with envelope-key rotation and loss of server-side JSON querying; do not reuse
secret injection or imply that cache encryption makes credentials appropriate
cache data.

### Storage Choice and Extraction Criteria

Choose storage from the consumer access pattern, not only record count:

| Need | Use |
| --- | --- |
| Shared point/multi-ID reads, bounded scans, independent refreshes, and Attune-scoped authorization | Data Cache API |
| Whole immutable snapshot downloaded once for local SQL/joins or disconnected batch work | Versioned file-backed SQLite artifact built from a pinned cache generation |
| Authoritative mutation, arbitrary querying/joins, analytics, long-term history, or non-Attune consumers | Independent operational database, warehouse, search system, or object/data-lake platform |

SQLite is a derived distribution format, not another authoritative store. Use
it only when consumers benefit from downloading the whole snapshot and can pin
an immutable artifact version. Do not use it for shared concurrent mutation or
when most consumers need a few records.

The cache may initially share Attune's PostgreSQL cluster. Moving its tables to
a dedicated PostgreSQL cluster is the preferred intermediate scaling step when
cache I/O, WAL, backup volume, or maintenance contention affects the Attune
control plane. The API, authorization, generation, quota, and retention
contracts stay unchanged; consumers must not connect to either database
directly.

Operators must define deployment-specific capacity and extraction thresholds
before enabling large refreshes. Review at least:

- cache table/index/retained-generation bytes against the allocated database
  capacity, including staging headroom;
- refresh and cleanup write rate, WAL volume, replica lag, checkpoint pressure,
  dead tuples, autovacuum health, and cleanup backlog age;
- cache API latency/error rates, seal/promotion duration and failures, stale
  namespaces, quota rejections, and expired-snapshot responses;
- backup size/duration and demonstrated restore time against the deployment's
  RPO/RTO, including cache data in WAL, replicas, and backups.

Escalate from tuning to a dedicated PostgreSQL cluster when a capacity warning
persists, cleanup cannot catch up within its configured cycle budget, backup or
restore objectives are at risk, or cache work materially degrades control-plane
SLOs. Extract the workload to an independent data system when it requires
authoritative retention or general querying, serves primarily non-Attune
consumers, cannot be rebuilt, or still exceeds its SLO/capacity envelope after
isolation. Threshold values are environment-specific and must be recorded in
the deployment runbook; record count alone is not an extraction criterion.

Aggregate admission is enforced in addition to per-generation and per-namespace
quotas. `cache_admission` limits live namespaces globally and per canonical
owner, physical entry bytes globally and per owner, and unpublished
(`staging` plus `ready`) generations per owner. Physical bytes include entries
in every retained generation state and in tombstoned namespaces until bounded
cleanup actually deletes them; logical retirement, failure, or tombstoning does
not immediately recover quota.

Namespace creation, generation creation, and chunk ingestion take one shared
PostgreSQL transaction advisory lock before measuring and admitting aggregate
usage. This serializes competing writers across API instances, and a rejected
chunk rolls back its entries and counters. Quotas reject new growth without
evicting or changing published snapshots. Exact idempotent generation/chunk
replays return the already accepted result, which keeps retry behavior stable
when usage reaches a limit. Rejections expose stable codes for global/owner
namespace limits, global/owner physical-byte limits, and the owner unpublished-
generation limit so clients and telemetry do not parse messages. See the
configuration guide for fields and defaults.

## SQLite Artifact Alternative

Artifacts are viable when a job needs an entire, read-only snapshot rather
than server-side point lookup. Artifact versions are immutable, and Attune
serializes version-number allocation with a PostgreSQL advisory transaction
lock. A producer can build a SQLite database off-line, validate it, publish a
new artifact version, and have readers pin that explicit version.

This does not require a reader/writer lock if the SQLite file is never
modified after publication. It does require an atomic publication contract:

1. Build and validate a new file in a private staging location outside the
   currently published version.
2. Publish only completed content through an artifact finalize/ready mechanism
   or a manifest-pointer convention that readers use as the sole discovery
   path.
3. Readers use a specific ready version rather than "latest" while a new file
   is being produced.

The current file-backed artifact representation has no ready-state marker, so
allocating a file-backed version and then writing its path is not atomic
publication. The existing multipart upload path also buffers a bounded file
and stores content in PostgreSQL, so it is not automatically the desired path
for a large SQLite export. The optional export therefore requires either:

- an artifact-version `staging`/`ready` finalize contract whose list/latest/
  download paths exclude staging versions, with an atomic same-filesystem
  rename or transport commit before marking ready; or
- a completed data version plus a separately published manifest pointer that
  is updated only after validation, with consumers forbidden from discovering
  exports through the data artifact's unqualified latest version.

SQLite artifacts require every consumer to download the whole database and
provide no shared API for namespace enumeration or point lookup.

Use this approach for infrequently refreshed, whole-dataset workloads. Use the
dedicated cache subsystem when jobs need shared record lookup, cursor scans,
or independently refreshed namespaces.

## Feature Implementation Plans

This section describes the implementation work for each proposed mechanism.
The cache API must not be exposed until its storage, authorization, and
generation-pinning behavior are all present.

### 1. Owner-Scoped Namespaces

**Intent**

Make each dataset a separately addressable and independently governable unit
within an existing Attune owner scope. This avoids global-name collisions,
prevents unrelated datasets from sharing refresh lifecycle, and gives
authorization a stable boundary.

**Implementation**

1. Add a `cache_namespace` model in `crates/common/src/models.rs` and a
   `CacheNamespaceRepository` in `crates/common/src/repositories/cache.rs`.
2. Add a migration with `BIGINT` primary keys, the same owner-type and
   canonical owner-ID/owner-ref fields used by keys, a normalized `namespace`,
   lifecycle timestamps, freshness/limit fields, and the eventual
   active-generation pointer.
3. Enforce exactly one matching canonical owner ID for non-system scopes and
   none for system. Enforce one namespace per canonical owner scope using a
   generated owner key, `NULLS NOT DISTINCT`, or partial unique indexes; do not
   rely on nullable-column uniqueness.
4. Add API DTOs and routes under `crates/api/src/dto/cache.rs` and
   `crates/api/src/routes/cache.rs`. Protect every route with `RequireAuth`.
   Namespace names should use one documented lowercase ASCII format and length
   limit, for example `^[a-z0-9][a-z0-9._-]{0,127}$`, and be normalized at the
   API boundary.
5. Resolve API owner refs through existing repositories to canonical IDs
   before authorization or creation. Keep SQL unqualified and all database
   access in cache repositories.

**Interactions**

- Namespace ownership is the common boundary for RBAC, execution-token access,
  retention, auditing, and all record queries.
- API paths may use `owner_type` plus `owner_ref` for ergonomics, but repository
  queries and uniqueness use canonical IDs. System has no owner ref; identity
  selectors should normally resolve to the authenticated identity unless an
  explicit cross-identity grant allows otherwise.
- A single scope may contain independent namespaces such as `users`,
  `locations`, `cost_centers`, and `management_organizations`.
- Namespace creation must precede generation creation; it is the stable parent
  for refreshes and for external clients.

### 2. Immutable Generations and Entries

**Intent**

Separate preparation from visibility. Immutable generations let writers build
a replacement without disturbing readers and make a published result an
identifiable, reproducible data snapshot rather than a moving collection of
rows.

**Implementation**

1. Add `cache_generation`, `cache_entry`, and resumable-ingest metadata tables
   and corresponding models and repositories. Generation state should be a
   PostgreSQL enum or equivalent constrained field: `staging`, `ready`,
   `active`, `retired`, or `failed`.
2. Store each record's external identifier as `TEXT` and its payload in
   `JSONB`. Enforce identifier/value byte limits. Add the unique bytewise
   `(generation_id, external_id)` index; it serves both point lookups and
   ordered scans.
3. Store expected and actual record/byte counts, optional source revision and
   versioned checksum, creator/client-refresh identity, lifecycle timestamps,
   and `readable_until` on generations. Keep the namespace's
   `active_generation` as the single source of truth for current reads and
   validate that it belongs to that namespace.
4. Define repository `SELECT_COLUMNS` constants for evolving models and use
   them for all SQLx reads. Keep all database access in repositories and leave
   SQL unqualified so the test and runtime search paths continue to work.
5. Make state transitions repository operations, not generic patch updates.
   Entry insertion locks or conditionally checks the generation state in the
   same transaction; only `staging` accepts writes, only `ready` promotes, and
   published entries are immutable.

**Interactions**

- Entries never move between generations. A refresh creates a complete new
  generation rather than mutating the active one.
- The generation ID becomes the consistency token used by pagination,
  multi-namespace snapshots, retention, and artifact manifests.
- The `ready` seal closes the validation/write race and makes promotion a
  short metadata transaction rather than a data-validation transaction.
- Payload schema remains application-defined. Do not add JSONB indexes until
  a demonstrated query needs one.
- New cache tables do not need execution/worker history triggers. Add cache
  lifecycle audit events instead unless a concrete record-history requirement
  emerges.

### 3. Exact Lookup, Multi-ID Lookup, and Cursor Scans

**Intent**

Support the three practical cache consumption patterns without making
callers choose between 200,000 individual HTTP requests and loading an
unbounded response into memory: one record, a known set of records, or a full
dataset traversal.

**Implementation**

1. Add repository methods to resolve the active generation, fetch one entry by
   `(scope, namespace, external_id)`, fetch a bounded set of external IDs, and
   scan a requested generation after a cursor.
2. Define a versioned opaque cursor containing the namespace/generation IDs,
   last external ID, page shape, and expiration. Integrity-protect it or
   revalidate every field as untrusted input. Reject malformed, expired, or
   mismatched cursors.
3. Add read endpoints for point lookup, bounded multi-ID lookup, and list
   traversal. Put point/multi-ID identifiers in request bodies where practical
   to avoid access-log leakage. Enforce page item and serialized-byte limits.
4. Return the selected generation ID, next cursor, record count when known,
   cursor expiration, and freshness metadata in every list response.
5. Use bytewise keyset ordering on the indexed external ID, never offset
   pagination. Preserve the request order or return an explicit per-ID result
   map for multi-ID lookups so missing IDs are unambiguous.
6. Optionally add an NDJSON full-scan endpoint after the cursor contract is
   stable. It must pin one generation, stream database rows with bounded
   buffering, apply a response byte/rate limit, and stop cleanly on disconnect.

**Interactions**

- Point lookup resolves the namespace's active generation unless the caller
  explicitly pins a still-retained generation.
- Cursor scans use the immutable generation from the first page, so a
  promotion cannot cause skipped or duplicated records.
- Multi-ID lookup prevents a workflow from making hundreds of point requests,
  while its bound protects the API and database from unbounded `IN` queries.
- The paged API is required initial functionality. The single-response NDJSON
  scan is an optimization for whole-dataset consumers, not a prerequisite for
  correctness.

### 4. Generation-Pinned Snapshots

**Intent**

Guarantee that a multi-request traversal answers "what records were published
in this dataset?" rather than returning an accidental mixture of old and new
refreshes. This is the consistency contract that makes the cache safe for
bulk action and sensor processing.

**Implementation**

1. Make the first namespace list request resolve and return the active
   generation. Require the returned generation in subsequent page requests.
2. Add an explicit read path for a supplied generation that verifies the
   generation belongs to the namespace and remains before `readable_until`.
3. Return a clear expiration response if a pinned generation has been
   removed; never silently switch a caller to the newer active generation.
4. Document the client contract: save the generation and cursor before
   fetching the next page, and retry the complete traversal when the pinned
   generation has expired.
5. Define and configure a maximum traversal duration. Promotion gives the
   retired generation at least that full remaining availability window, and
   cursor expiration never exceeds it.

**Interactions**

- This feature relies on immutable entries and retention. Without both,
  pagination cannot make a coherent snapshot promise.
- A bulk promotion changes only the active-generation pointer, so readers
  already pinned to the prior generation continue safely.
- Freshness expiration does not invalidate an active traversal; only the
  advertised snapshot availability boundary does.
- This mechanism is the foundation for an optional multi-namespace snapshot.

### 5. Bulk Ingestion, Validation, and Atomic Promotion

**Intent**

Make a 200,000-record refresh efficient and safe. The system must accept
large inputs in bounded pieces, validate the complete candidate dataset, and
change reader-visible state once rather than exposing partially loaded rows.

**Implementation**

1. Add APIs to create a staging generation, stream records into it, complete
   validation/sealing, promote it, and abandon it. Use bounded numbered NDJSON
   chunks with explicit record and byte limits, a client refresh ID, and an
   expected chunk count declared at create or seal.
2. Make chunk upload idempotent: the same generation/chunk index and request
   digest may be replayed as success; a different digest for an accepted index
   is a conflict. Commit each chunk atomically so a dropped HTTP connection has
   an unambiguous retry result.
   Reusing a client refresh ID with different create parameters is likewise a
   conflict, not a second generation.
3. Implement bounded server-side batches with SQLx `QueryBuilder` or a
   dedicated PostgreSQL copy path. Do not issue one database transaction or
   HTTP round trip per record, and do not hold one transaction open while an
   untrusted client streams all 200,000 records.
4. Detect duplicate external IDs through the unique constraint and return an
   actionable ingestion error. Duplicate rows inside a new chunk fail that
   chunk; identical accepted-chunk replay is handled by chunk idempotency.
5. Seal in a transaction that locks the generation, changes it from `staging`
   to `ready`, prevents new chunks, verifies exactly the contiguous chunk range
   `0..expected_chunk_count`, computes authoritative count/size, and validates
   expected values and any supported checksum.
6. Promote in a short transaction that locks the namespace row, verifies the
   expected active generation and quota state, updates `active_generation`,
   sets the new generation active, retires the prior generation, and sets its
   minimum `readable_until`.
7. Mark incomplete or failed staging generations explicitly and make them
   unreadable. Expire abandoned staging generations automatically.

**Interactions**

- Staging generations are intentionally invisible to point lookup and scans.
- Ready generations are also invisible until promotion; sealing makes their
  content immutable.
- The required expected-active-generation value prevents a slow writer from
  promoting over a newer completed refresh.
- Client refresh and chunk idempotency make network retries practical without
  weakening duplicate-ID validation.
- Promotion emits one summary audit event and provides the generation used by
  subsequent read and retention operations.

### 6. Refresh Concurrency and Multi-Namespace Snapshots

**Intent**

Prevent competing refresh jobs from losing a newer result, while avoiding
coordination unless it is needed. Most datasets should refresh independently;
only business workflows requiring relationships across datasets should pay for
a coordinated multi-namespace snapshot.

**Implementation**

1. Start with optimistic concurrency on a namespace: callers provide the
   active generation observed when they began their refresh; promotion fails
   when it has changed.
2. Add a short-lived refresh lease only if optimistic retries prove
   insufficient for an integration. Leases need expiration and cleanup so a
   crashed producer cannot block future refreshes.
3. If related namespaces must be mutually consistent, add a
   `cache_refresh_set` and `cache_refresh_set_member` model. A completed set
   immutably maps each namespace to one ready generation and records the
   expected prior active generation for each member.
4. Promote a refresh set atomically only after every member generation is
   complete. Lock namespace rows in deterministic ID order to avoid deadlocks,
   verify every expected prior generation, update all pointers in one
   transaction, and fail the whole set on any mismatch. Readers can pin the
   refresh-set ID and derive a fixed generation for each dataset.

**Interactions**

- Normal use remains per namespace: Users, Locations, Cost Centers, and
  Management Organizations can refresh independently under one scope.
- A refresh set is optional because it adds coordination cost and should be
  used only when cross-dataset referential consistency is required.
- Retention must preserve every generation referenced by an unexpired refresh
  set, even if it is no longer the namespace's active generation.

### 7. RBAC, Execution Tokens, and Sensor Tokens

**Intent**

Treat cached business data as protected data with a different threat model
from credentials. A caller should receive only the namespaces it is entitled
to read or refresh, and neither a broad key permission nor a worker/sensor
token should silently expand that authority.

**Implementation**

1. Add a proposed `Resource::Caches` RBAC resource and use existing CRUD-style
   actions: `Read` for entries and scans, `Create` for namespaces and staging
   generations, `Update` for ingestion and promotion, and `Delete` for
   explicit deletion. Do not reuse `Resource::Keys`.
2. Represent a specific namespace through the existing `refs` constraint by
   setting the authorization target ref to the normalized namespace name and
   combining it with owner-type/owner-ref constraints. If generic refs cannot
   express this cleanly, add a cache-specific namespace constraint. Owner-only
   grants intentionally cover all namespaces in that owner; writers should
   normally include the namespace ref. Compile the supported constraint subset
   into SQL predicates in the cache repository, following the key-listing
   fail-closed pattern.
3. Extend `execution_standard_access_grants` so `standard` grants
   read-only cache access to the action/pack refs already signed into the
   execution token, including containing workflow refs where applicable. It
   must not grant write or cross-pack access, and it exists only when the
   execution requested non-empty `permission_set_refs` containing `standard`.
4. Extend sensor token minting so the API resolves the registered sensor and
   signs its exact sensor/pack refs plus an explicit cache permission snapshot
   or permission-set refs. Cache routes evaluate only that signed authority.
   Do not infer the sensor identity's ordinary roles, parse authority from an
   unverified request field, or inherit the current key-route bypass for
   sensor/worker tokens.
5. Reject refresh and worker tokens on cache routes by default. Apply
   authorization before namespace existence/freshness lookup and use a
   consistent not-found/forbidden shape to avoid existence leakage.
6. Emit cache audit events for namespace changes, generation lifecycle events,
   denied operations, and read summaries when policy requires them. Never
   include entry payloads or raw external-ID lists. Do not emit one event per
   record during a bulk scan or upload.

**Interactions**

- Authorization must be applied before resolving the active generation. A
  caller must not learn whether an inaccessible namespace is fresh or exists.
- Namespace ownership supplies the authorization context for every generation
  and entry; generations do not carry independent ownership.
- List/search/count predicates must be authorization-aware in SQL and use the
  same filtered dataset. In-memory post-filtering or token-type bypasses would
  leak counts and will not scale.
- Action and sensor HTTP consumers depend on this work before they can safely
  access the cache API.

### 8. Action and Sensor HTTP Access

**Intent**

Keep cache use explicit and demand-driven. Workers must not deserialize a
large namespace for every execution, and action authors need one reliable
HTTP contract that carries scoped credentials, preserves snapshot semantics,
and is consumable through generated SDKs.

**Implementation**

1. Expose the cache HTTP contract through the API service and document it in
   OpenAPI so pack actions and managed sensors can use generated SDKs for
   their runtime language. Clients read `ATTUNE_API_URL` and
   `ATTUNE_API_TOKEN`, request only needed records, and support generation
   pinning.
2. Generated SDKs must support point read, bounded multi-ID read, full scan,
   and refresh lifecycle calls. Scan consumers must process a page at a time
   rather than accumulating an entire namespace in memory.
3. Update action and sensor authoring documentation with examples that use
   generated SDKs. Make clear that cache values are delivered over the API,
   not as ambient action parameters.
4. Keep credentials for the upstream system in Keys and Secrets. The cache
   HTTP API does not replace secret delivery.
5. Make missing-token behavior explicit: an action with empty
   `permission_set_refs` has no `ATTUNE_API_TOKEN`; generated clients return a
   clear configuration error rather than falling back to direct database or
   unauthenticated access.
6. Bound retries and total traversal time. On snapshot expiration the client
   restarts from page one only when the caller opts into that behavior; it
   never combines pages from two generations.

**Interactions**

- The client combines scoped execution or sensor tokens with the generation
  identifier from the read API.
- Bulk refresh actions use the same client to create, upload, validate, and
  promote generations.
- This isolates high-cardinality data from the worker's existing secret stdin
  channel and allows per-request authorization.

### 9. Retention, Freshness, and Operations

**Intent**

Balance correctness with bounded storage. Readers need old generations long
enough to complete pinned traversals, while operators need stale data,
failed refreshes, and unbounded historical storage to become visible and
actionable.

**Implementation**

1. Add namespace-level freshness fields and generation expiration. Expose
   refresh time, source revision, active record count, and stale state in
   namespace responses.
2. Add a supervisor maintenance loop that retires expired staging generations,
   deletes retained generations only after their availability window, and
   removes entries/generations in bounded batches.
3. Ensure cleanup excludes active generations and generations referenced by
   unexpired published refresh sets. Use advisory leadership in the
   supervisor, consistent with other maintenance loops.
4. Enforce hard admission quotas as well as cleanup: record/value/chunk limits,
   maximum concurrent staging generations, generation/namespace bytes, retained
   generation count, and aggregate bytes per owner and deployment. Retention
   alone cannot protect PostgreSQL from one producer, or many individually
   valid namespaces, uploading faster than cleanup.
5. Add metrics for namespace age, last successful refresh, record count,
   staging duration, promotion failures, expired-pinned-generation responses,
   quota rejections, cleanup backlog, and storage consumed per scope.
6. Emit an alert when a namespace exceeds its freshness target or a staging
   generation repeatedly fails.

**Implemented telemetry:** Attune does not currently expose a shared in-process
metrics registry, so the cache uses the existing structured tracing convention
instead of introducing a cache-only framework. The supervisor emits bounded
`cache_maintenance_cycle` and `cache_scope_storage` metric sets for ages,
counts, failures, cleanup backlog, and per-owner-type storage. The API emits
`cache_api_outcomes` counter events for promotion failures, expired snapshot
responses, and quota rejections. These fields are suitable for log-derived
counters/alerts and contain only low-cardinality operation/status fields and
numeric namespace/generation IDs, never external IDs or values.

**Interactions**

- Retention enforces the availability contract promised by generation-pinned
  pagination.
- Freshness data allows actions to decide whether to use stale data, trigger a
  refresh, or fail safely.
- Cleanup must understand refresh sets before that optional feature can be
  enabled in production.
- Do not rely on deleting a generation with 200,000 cascading entry deletes in
  one transaction as the routine cleanup path. Delete entries by indexed
  bounded batches, then delete the empty generation. This limits locks, WAL
  bursts, replication lag, and table bloat; the foreign-key cascade remains a
  safety net rather than the normal algorithm.
- Cache maintenance belongs in repository methods called by the supervisor,
  not ad hoc SQL in the service.
- Namespace and owner deletion must use the same tombstone-and-batched-cleanup
  path. Avoid owner-to-cache cascades that can synchronously delete millions
  of rows; owner foreign keys should restrict deletion until cache cleanup
  completes.
- Metrics must never use external IDs as labels. Keep owner/namespace label
  cardinality bounded; expose detailed per-namespace status through the
  database/API when fleet-wide metric labels would become excessive.

### 10. SQLite Artifact Snapshot Option

**Intent**

Offer a compact offline format for workloads that truly consume an entire
dataset without weakening the shared cache's lookup, authorization, and
publication guarantees. SQLite is a derived, reproducible export, not a
concurrent shared mutable store.

**Implementation**

1. Treat SQLite as an optional consumer format, not as the canonical cache
   store. Build it from a completed, pinned cache generation or refresh set.
2. Write the database to a private staging path, validate SQLite integrity,
   schema version, expected row counts, and checksum, then publish a new
   artifact version with a manifest containing source namespace/generation
   IDs, export schema version, record counts, and checksum.
3. Prefer an artifact-version finalize contract: allocate staging, write,
   validate, atomically commit the file/object, then mark the version ready.
   List/latest/download must exclude staging versions. If a manifest-pointer
   convention is used instead, readers discover the data version only through
   the completed manifest and never through unqualified data-artifact latest.
4. Make consumers pin the artifact version and verify its manifest before
   opening it. Retain the source cache generations for at least as long as the
   artifact version needs traceability.
5. Build/export in streaming batches and create SQLite indexes after bulk row
   insertion where practical. The exporter must not materialize 200,000 JSON
   records in memory.

**Interactions**

- This provides efficient whole-dataset delivery for offline or batch jobs,
  while the cache API remains the source for shared point lookup and scans.
- The manifest links artifact versions to cache generations, making results
  reproducible and debuggable.
- It has no role in the normal cache write path; it is a derived export.
- The artifact and cache permissions both apply: permission to read the cache
  does not automatically grant the derived artifact, and artifact access must
  not reveal a source generation the caller could not otherwise identify.

## CLI and Web Client Experience

### Intent

Expose cache data as a distinct, scoped, generation-based resource without
making it look like an unusually large secret. Operators need to find a
namespace, inspect its health, look up specific records, and safely control a
refresh. They must not be encouraged to edit active records in place, dump
200,000 values accidentally, or mistake cache access for secret decryption.

### Preserve the Keys and Secrets Boundary

Do not add `namespace`, `external_id`, generation, freshness, retention, or
bulk-upload fields to `attune key` or to the existing **Keys & Secrets** web
page. Those clients remain focused on small mutable configuration values and
credentials:

- `attune key show --decrypt` remains a privileged secret-value operation.
- The existing `SecretsService`, `useKeys` hooks, `/keys` route, and key
  create/edit forms remain Key-only. In particular, cache data has no
  encryption checkbox, key reference, or inline value editor.
- Add `Resource::Caches` rather than extending `Resource::Keys`; add matching
  cache grants to the API and to the web permission model. Cache read access
  does not imply key metadata or key decryption access, and the converse is
  also true.

The web navigation should expose a sibling **Data Caches** item at `/caches`,
not a tab or mode inside **Keys & Secrets**. This preserves the user's mental
model: keys provide input configuration to work; caches provide versioned
external business data that work can query.

### CLI Command Group

Add a top-level `attune cache` command group beside `attune key`. Implement it
in a dedicated `crates/cli/src/commands/cache.rs` module, wire it through
`commands/mod.rs` and `main.rs`, and use separate API request/response types.
It must never reuse the key command's value or decrypt behavior.

All commands except `namespace list` require an explicit owner selector.
`namespace list` lists all accessible namespaces by default and accepts the
same owner flags to restrict the result. Prefer typed owner flags such as
`--owner-type pack --owner-pack-ref salesforce` over an opaque owner ID; the
API resolves the canonical owner ID. Require an explicit `--owner-type system`
for a system-owned namespace rather than silently defaulting cache writes to
the system scope.

The initial command tree should support:

```text
attune cache namespace list
attune cache namespace create <namespace>
attune cache namespace show <namespace>
attune cache namespace update <namespace>
attune cache namespace delete <namespace> --yes

attune cache entry get <namespace> <external-id>
attune cache entry get-many <namespace> --external-id <id> [...] [--external-id-file <path>]
attune cache entry scan <namespace>

attune cache generation list <namespace>
attune cache generation show <namespace> <generation-id>

attune cache refresh begin <namespace>
attune cache refresh upload <namespace> <generation-id> --chunk-index <n> --file <ndjson>
attune cache refresh seal <namespace> <generation-id>
attune cache refresh promote <namespace> <generation-id> (--expected-active <id> | --expect-empty)
attune cache refresh abort <namespace> <generation-id> --yes
attune cache refresh apply <namespace> --input <ndjson>
```

`namespace create` and `update` manage namespace-level policy only:
freshness target, record/byte quotas, and retention limits. Owner scope and
namespace are immutable; changing either creates a new namespace. `namespace
show` reports the active generation, freshness/staleness, counts, byte usage,
configured limits, and refresh health without reading any entries.

`max_retained_generations` must be at least `2`, reserving the active snapshot
and one prior snapshot for pinned readers. `freshness_target_seconds: 0`
disables freshness evaluation. Pack-managed namespaces cannot be updated or
deleted through these namespace commands; update their pack definition.

The explicit `refresh begin/upload/seal/promote` commands support recovery and
automation. They expose the generation ID, client refresh ID, expected active
generation, chunk count, source revision, and validation result. They do not
retry a conflicting promotion or silently force a newer generation aside.
`refresh apply` is an ergonomic wrapper for the normal lifecycle: it streams
an NDJSON file into bounded numbered chunks, seals, and promotes using the
same optimistic expected-active value. It must not load the input file or all
responses into memory. A force promotion, if introduced, is a distinct
strongly authorized command with an explicit confirmation and audit reason.

Entry commands are deliberately bounded:

- `entry get` returns one full value after a deliberate point lookup.
- `entry get-many` accepts a bounded set of IDs, preferably from a file or
  stdin when IDs are sensitive, rather than placing a long ID list in a URL
  or shell history.
- `entry scan` defaults to one cursor page and reports the pinned generation,
  cursor expiry, and next cursor. `--generation` and `--cursor` allow an
  operator to resume the same snapshot.
- `entry scan --all --output ndjson` is an explicit streaming convenience
  mode. Extend the CLI output enum with `ndjson` for this command only; emit
  records one at a time to stdout and write snapshot metadata to stderr. Reject
  a whole-dataset JSON/YAML/table materialization. A snapshot-expired response
  must fail clearly rather than restarting against the latest generation.

Human table scans should show external ID, source timestamp, record size, and
a compact value indicator by default. Full record values require `entry get`,
`--include-values`, or an explicit machine-readable scan mode. This is a
usability guard against incidental disclosure and terminal flooding, not a
replacement for cache RBAC. Cache values are not decrypted and should never
use the key command's SHA-256 "value hidden" convention.

The CLI already honors an execution-scoped `ATTUNE_API_TOKEN`; this allows an
action that explicitly requested cache permissions to use the same commands or
a generated SDK. An action without such a token must receive the documented
authorization/configuration error. Normal action and sensor code should use
generated SDKs from the cache OpenAPI contract for paging and retry behavior;
the CLI is an operator and automation interface, not the worker's ambient data
delivery path.

### Web Application

Regenerate the web OpenAPI client after cache routes exist, yielding a distinct
cache service and cache DTOs. Add cache React Query hooks such as
`useCacheNamespaces`, `useCacheNamespace`, `useCacheGenerations`,
`useCacheEntries`, and refresh lifecycle mutations. Cursor query keys must
include namespace, generation, cursor, and page shape so a promotion cannot
mix cached pages from different generations.

Add `/caches` and `/caches/:ownerType/:ownerRef/:namespace` routes, a
**Data Caches** navigation item, and a `caches` requirement in
`web/src/lib/permissions.ts`. Do not add `caches` to the authenticated default
read resources: the navigation and controls may be hidden or disabled for
convenience, but the API remains the authority for every request and scoped
result.

The namespace index should use server-side scope/namespace/freshness filters
and show only metadata:

| Column | Why it matters |
| --- | --- |
| Owner scope and namespace | Identifies the authorization and dataset boundary. |
| Status | Distinguishes uninitialized, fresh, stale, refreshing, and failed state. |
| Active generation and source revision | Makes a published snapshot traceable. |
| Records and bytes | Makes scale and quota consumption visible without loading entries. |
| Last successful refresh / freshness target | Lets an operator diagnose stale upstream data. |
| Retention and staging state | Explains whether old snapshots remain available and whether a refresh is stuck. |

The detail page should have focused tabs rather than one editable record grid:

1. **Overview** shows namespace policy, freshness, active generation,
   quotas/usage, last refresh outcome, and links to relevant audit events.
2. **Records** supports exact external-ID lookup, bounded multi-ID lookup, and
   cursor-page browsing. It prominently shows the pinned generation and
   snapshot expiry, with a **Restart on current generation** action when a
   cursor expires. It must not offer arbitrary JSONB filters, offset paging,
   or a default "load all records" operation.
3. **Generations** lists staging, ready, active, retired, and failed
   generations with counts, bytes, source revision, checksum metadata,
   creator, and lifecycle timestamps. A generation detail can show ingest
   chunk status and seal errors, but not edit its entries.
4. **Refresh** permits an authorized writer to create/resume a refresh, select
   a local NDJSON file, stream it in bounded chunks with progress and retry
   state, seal it, and review the old/new generation summary before promotion.
   Browser ingestion is a controlled manual workflow; scheduled 200,000-record
   synchronizations should normally run through an Attune action or CLI.

The page must make destructive and publication actions explicit:

- Promotion displays the expected active generation and refuses to hide a
  conflict. A force promotion is visually distinct, requires its elevated
  grant and reason, and is unavailable by default.
- Abandoning staging data and deleting a namespace show count/byte impact and
  use a tombstone/queued-cleanup status instead of claiming a large deletion
  is instantaneous.
- Cache values are never shown in the namespace index, generation summaries,
  toast messages, or audit previews. A record value is visible only after an
  explicit Records lookup to a caller with cache read scope.

Extend the permission-set editor to expose the separate `caches` resource and
scoped owner/namespace reference constraints. The interface should make the
read-versus-write distinction clear: standard execution access is read-only,
while refresh creation, upload, sealing, promotion, and deletion require
explicit cache write grants.

### MCP Cache Tool Policy

The MCP server may expose the existing bounded cache tool surface under the
same API authorization rules as other cache clients. This includes namespace
and generation metadata, point and bounded multi-ID reads, exactly one bounded
cursor-scan page per MCP call, and a structured refresh lifecycle performed
over multiple calls. Scan responses preserve the API's generation and cursor,
and entry values remain opt-in. Refresh calls separately begin a generation,
upload one bounded structured chunk, seal, optimistically promote, or abort it.

MCP adds no cache authority of its own. Every call uses ordinary cache API RBAC;
reads require the applicable cache-read authority, and refresh creation,
upload, sealing, promotion, and abort require explicit grants for those cache
write operations and owner/namespace scope.

The MCP surface must continue to exclude automatic cursor-following and any
full-dataset response, the CLI's `entry scan --all` behavior, filesystem-based
`refresh apply`, and force promotion. Clients may coordinate bounded scan or
refresh calls, but each call remains independently bounded, authorized, and
subject to the API's generation, quota, and optimistic-concurrency contracts.

### Interactions and Delivery

- API route and RBAC work comes first because both CLI and web clients must
  use the same owner/namespace predicate and generation-pinned contracts.
- The CLI can deliver with the initial API and is the preferred operational
  interface for large imports, resumable retries, and script automation.
- The web client delivers a safe operator experience: namespace policy,
  health, point inspection, and an optional bounded manual refresh flow. It
  must not become a second bulk-ingestion protocol.
- Actions and sensors consume the cache through scoped tokens and generated
  SDKs backed by the OpenAPI contract; their cache activity is visible through
  generation status and summary audits, not through secret injection views.
- The MCP server remains subordinate to the canonical bounded-operation and
  authorization policy above; it does not create a parallel cache protocol.

## Initial Scope Versus Optional Extensions

The initial production-capable cache includes owner-scoped namespaces,
canonical owner validation, immutable staging/ready/active generations,
idempotent bounded ingestion, atomic optimistic promotion, exact and bounded
multi-ID lookup, generation-pinned cursor scans, access/execution/sensor
authorization, quotas, minimum retired-generation availability, supervisor
cleanup, summary auditing, and operational metrics. These mechanisms are
required together because omitting any one weakens correctness, access control,
or bounded operation for the stated 200,000-record use case.

The initial operator surface is a separate `attune cache` CLI group and
**Data Caches** web area for namespace policy, refresh lifecycle, generation
health, and bounded record access. Keys and Secrets remains unchanged. The
browser's manual import flow is bounded and resumable, while scheduled
large-scale refreshes use an action or the CLI.

Optional later features are refresh leases beyond optimistic concurrency,
cross-namespace refresh sets, a single-response NDJSON scan endpoint, arbitrary
JSONB secondary indexes, canonical whole-generation content checksums, and
SQLite artifact exports. Optional features must build on the same generation,
authorization, retention, and quota contracts rather than creating a parallel
cache path.

## Feature Dependencies

| Feature | Depends on | Enables |
| --- | --- | --- |
| Owner-scoped namespaces | owner validation, migrations, repositories | RBAC context, generation parentage |
| Generations and entries | namespaces, state constraints, indexes | immutable reads, staging refreshes |
| Cursor scans and lookup | generations and entry indexes | efficient action and sensor reads |
| Pinned snapshots | immutable generations and retention | complete, coherent traversal |
| Bulk refresh and promotion | staging/ready states, idempotent chunks, namespace lock, quotas | atomic refresh visibility |
| RBAC and token grants | namespaces and cache routes | safe API access for users, actions, and sensors |
| OpenAPI-generated SDKs | routes, RBAC, pinned snapshots | deliberate cache consumption and refresh |
| CLI and web clients | API contracts, RBAC, pinned snapshots | safe operator lifecycle, inspection, and automation |
| Retention and observability | generations, snapshot contract, quotas | bounded storage and freshness operations |
| Refresh sets | pinned snapshots and retention | cross-namespace consistency |
| SQLite export | pinned snapshots and artifact readiness | offline whole-dataset consumption |

## Delivery Order

1. Implement models, migrations, repository methods, and repository tests for
   namespaces, generations, entries, ingest chunks, owner validation, state
   transitions, quotas, and indexes. Use unqualified SQL and model
   `SELECT_COLUMNS`. Run `cargo sqlx prepare` after the schema work.
2. Implement read-only API routes with cache RBAC, exact lookup, multi-ID
   lookup, and generation-pinned cursor scans. Protect routes with
   `RequireAuth`, filter in repository SQL, and add API/authorization tests
   before allowing any writer.
3. Implement idempotent staging ingestion, sealing, quota enforcement,
   optimistic promotion, summary auditing, and 200,000-record load tests.
4. Add execution standard/named grants and explicit signed sensor authority,
   then document the cache HTTP contract in OpenAPI for action/sensor SDK
   generation. Confirm empty permission-set executions have no token and cache
   records never enter secret stdin.
5. Add the `cache` CLI group, generated web cache client, `Resource::Caches`
   permission controls, and the separate **Data Caches** UI. Test bounded
   scan output, refresh conflicts, route access, and cursor reset behavior.
6. Add minimum snapshot availability, bounded supervisor cleanup, metrics,
   freshness alerts, storage/bloat monitoring, and operational runbooks.
7. Add refresh sets only for integrations that require cross-namespace
   consistency. Add SQLite exports only for workloads that benefit from
   complete offline snapshots.

Required tests include 200,000-record streamed ingestion, duplicate-ID
rejection, identical and conflicting chunk replay, seal-versus-write races,
concurrent publishers, generation-pinned scans during promotion, bytewise
cursor ordering, page byte limits, exact scoped lookup, owner/namespace RBAC
isolation, execution tokens with and without `permission_set_refs`, sensor and
worker token scoping, quota rejection, bounded cleanup, expiration behavior,
and confirmation that cache records are not delivered as secrets. Tests for
refresh-set retention and artifact manifest/readiness validation are required
only when those optional features are implemented.

## End-to-End Test Strategy

### Intent

Repository and API tests can prove individual SQL predicates and state
transitions, but they cannot prove that an action or managed sensor receives
the intended token, calls the deployed API, and observes the same published
generation as another service. The E2E suite must validate those service
boundaries without turning a 200,000-record load test into a requirement for
every pull request.

### Harness and Test Placement

Use the Docker Compose lifecycle invoked by `make e2e-test` and
`scripts/run-integration-tests.sh`. It runs the API, executor, workers, sensor,
notifier, supervisor, PostgreSQL, and RabbitMQ together and executes the
Python suite in `tests/e2e/`.

Add cache scenarios using the existing tier layout:

| Test location | Purpose | Default cadence |
| --- | --- | --- |
| `tests/e2e/tier1/test_t1_09_cache_basics.py` | Essential namespace, refresh, and read contract | Every E2E run |
| `tests/e2e/tier2/test_t2_14_cache_snapshot_and_ingest.py` | Multi-page snapshots, idempotent chunks, promotion conflicts | Every E2E run |
| `tests/e2e/tier3/test_t3_24_cache_security_and_lifecycle.py` | Action/sensor authority, audit safety, retention, and failure paths | Tier 3 / security gate |
| `tests/e2e/tier3/test_t3_25_cache_load.py` | Full 200,000-record streaming load and recovery | Scheduled or manually selected performance gate |
| `tests/e2e/tier3/test_t3_26_cache_optional_exports.py` | Refresh sets and SQLite exports | Added only with those optional features |

Reserve the next available scenario numbers if the tier files change before
implementation. The grouping matters more than the number.

Add a `cache` pytest marker to `tests/pytest.ini`, which uses strict markers;
the existing `performance` marker identifies the full-scale scenario. Keep the
200,000-record test behind the `performance` marker and expose a dedicated Make
target such as
`make e2e-test-cache-load ARGS='-m cache and performance'`. The regular cache
tests should use smaller, representative datasets and remain in the normal
E2E gate.

After cache routes are added to OpenAPI, regenerate the Python client used by
the E2E suite and add cache methods to the wrapper client. During initial API
development, narrowly scoped raw client requests are acceptable, but the final
tests should use generated request and response types where available.

### Test Data, Isolation, and Cleanup

1. Create every namespace with `unique_ref()` under the test pack assigned to
   the pytest worker. Never use a shared system namespace or a real external
   identifier.
2. Use deterministic synthetic values and high-entropy sentinel strings. The
   sentinel is safe to search for in permitted test responses, logs, and audit
   metadata to prove that payloads and external IDs were not disclosed.
3. Exercise the cache only through its public API and through action/sensor
   API calls. E2E tests must not seed, alter, or inspect cache tables with
   direct SQL; repository tests own database-level setup and assertions.
4. Teardown through the cache lifecycle API and wait for the namespace to be
   deleted or tombstoned. The E2E test profile should use isolated data volumes
   and a short supervisor maintenance interval, but tests must not depend on
   broad timestamp-based database cleanup.
5. Preserve the namespace and client refresh IDs in assertion messages. On a
   failure, `make e2e-test-debug` leaves the Compose stack running so service
   logs and cache status can be inspected without reproducing the load.

### Required Tier 1 Contract Scenario

`test_t1_09_cache_basics.py` establishes the smallest complete vertical slice:

1. Create a unique pack-scoped `users` namespace with a small quota.
2. Create a staging generation, upload two or more numbered chunks, seal it,
   and promote it.
3. Assert one exact lookup and one bounded multi-ID lookup return the expected
   values and distinguish a missing external ID.
4. Start a cursor scan with a small page size; assert every ID appears once,
   in bytewise order, and the returned generation is the active generation.
5. Create a second namespace such as `locations` under the same owner and
   prove identical external IDs do not collide across namespaces.
6. Verify a list response contains values, unlike the Keys list endpoint, but
   does not expose credentials, secret delivery fields, or unrelated
   namespaces.

This validates that namespaces, entries, read routes, and the full Compose
stack agree on the basic cache contract before more complex concurrency and
authorization behavior is introduced.

### Required Tier 2 Refresh and Snapshot Scenario

`test_t2_14_cache_snapshot_and_ingest.py` validates the consistency and retry
guarantees that distinguish this system from a mutable key/value list:

1. Upload a generation over several chunks. Replay one chunk with the same
   request digest and assert success without a second copy; replay it with a
   different digest and assert conflict.
2. Attempt to seal with a missing chunk, duplicate external ID, incorrect
   expected count, and an over-quota value. Assert all failures leave the
   active generation unchanged and unreadable staging data undiscoverable.
3. Promote a first generation and begin a multi-page scan, retaining its
   generation ID and next cursor.
4. In a second client, stage and promote a changed generation for the same
   namespace. Resume the first scan and assert it returns only the original
   generation's remaining IDs; a new scan must return only the new generation.
5. Start two refreshes from the same expected active generation and race their
   promotions. Assert exactly one succeeds and the losing refresh receives the
   documented conflict/precondition result without replacing the winner.
6. Assert malformed, expired, cross-namespace, or modified cursors fail
   closed and do not return data.

The scenario should use short pages and dozens or hundreds of records, not
200,000. Its purpose is deterministic validation of interleavings, not
throughput measurement.

### Required Tier 3 Security and Lifecycle Scenario

`test_t3_24_cache_security_and_lifecycle.py` validates access at real service
boundaries:

1. Create same-named namespaces in two pack scopes and create a restricted
   test user, an authorized user, and an administrator. Assert list, lookup,
   count, and generation-metadata calls all use the same visibility boundary:
   the restricted user neither reads records nor learns whether the other
   namespace exists.
2. Assert an execution with `permission_set_refs: ["standard"]` can read only
   the executing action and pack scopes. Use a small shell or Python action
   that calls the cache API with `ATTUNE_API_TOKEN` and emits only an
   `cache-read-ok` marker and generation ID.
3. Run an otherwise identical action with empty `permission_set_refs`. Assert
   `ATTUNE_API_TOKEN` is absent and the generated API client reports its
   explicit configuration/authentication failure; it must not fall back to
   direct database access or ambient cache input.
4. Run a managed test sensor with its signed sensor authority. Assert it can
   read its allowed pack/sensor scope, cannot read another scope, and cannot
   promote a generation without an explicitly configured write grant.
5. Assert an action's stdin parameter document does not contain cache
   payloads unless the action fetched them itself. Check action output,
   execution metadata, audit events, and service-visible error data do not
   contain the synthetic sentinel, raw external-ID lists, or cache values.
6. Configure a short test-only freshness target and snapshot availability
   window through the E2E Compose configuration. Poll rather than sleep for
   supervisor cleanup. Assert active and pinned-readable generations are
   retained, expired staging/retired generations are removed in batches, and
   an expired cursor returns the documented expiration result.

The sensor case is required because sensor-token handling differs from
identity-based access/execution token authorization. It must use the real
sensor service rather than a hand-minted token that could bypass production
token construction.

### Scheduled 200,000-Record Load Scenario

`test_t3_25_cache_load.py` validates the stated scale without asserting
machine-specific throughput:

1. Generate 200,000 deterministic records with fixed-width bytewise external
   IDs and modest JSON payloads. Upload them in bounded chunks, such as 100
   chunks of 2,000 records.
2. Assert sealing reports exactly 200,000 records and the expected byte count;
   promote and verify health/readiness remain available throughout ingestion.
3. Perform representative point lookup, bounded multi-ID lookup, and a
   page-by-page full scan. Assert exact count, first/last ordering, no
   duplicates, and one pinned generation.
4. While a scan is in progress, stage and promote a small changed generation.
   Assert the pinned scan remains coherent and a fresh read selects the new
   generation.
5. Repeat an upload chunk after a simulated client retry and assert the final
   count is unchanged. Attempt a quota violation and assert it fails before
   promotion.
6. Record wall-clock duration, response/error counts, and service resource
   telemetry as artifacts for trend analysis. Use an environment-specific
   budget only in the scheduled performance environment; do not make a fixed
   elapsed-time threshold a portable functional assertion.

Run this test after schema migration changes, cache ingestion changes, and on
scheduled CI. It should be opt-in for local development and ordinary pull
requests because it deliberately produces substantial database and WAL load.

### Optional Feature Scenarios

Add optional tests only with their feature:

| Feature | E2E scenario |
| --- | --- |
| Refresh sets | Publish Users and Locations as one set, prove readers pin both generations together, reject incomplete sets, and retain all member generations until the set expires. |
| NDJSON scan endpoint | Disconnect a streaming client mid-scan, verify the server releases resources, then restart from a cursor or generation without mixing snapshots. |
| SQLite export | Build an export from a pinned generation, verify its manifest checksum/count/source generation, reject staging artifacts from `latest`, and query the downloaded SQLite file for expected rows. |
| Application-layer encryption | Verify authorized decryption, key rotation behavior, ciphertext absence from responses/logs, and that unsupported server-side JSON filtering is rejected explicitly. |

### Coverage Boundaries

The E2E suite proves the deployed API and service interactions. It is not the
only test layer:

- Repository tests validate owner constraints, bytewise index behavior, state
  transitions, batch cleanup selection, and SQL predicates under
  schema-per-test isolation.
- API integration tests validate response schemas, malformed inputs, cursor
  integrity, audit-event shape, and RBAC decisions with controlled fixtures.
- Unit tests validate cursor encoding, checksum canonicalization, client retry
  behavior, and generation-state transition rules.
- The E2E scenarios above validate that those pieces remain correctly wired
  across Docker services, real execution tokens, sensor tokens, workers, and
  supervisor maintenance.

## Review Findings: Gaps and Alternate Solutions

A codebase-grounded review validated this design against the real Attune
implementation. Every codebase-specific claim that was checked held up,
including the security-critical ones: the pack-scoped ambient stdin secret
delivery in `crates/worker/src/secrets.rs` (which would broadcast a large
pack-scoped cache to every action in the pack), the sensor/worker token
bypass on key routes in `crates/api/src/routes/keys.rs` (row-level RBAC
visibility is applied only for `Access`/`Execution` tokens), the `standard`
execution-access grant plumbing in `crates/api/src/authz.rs`, and the
advisory-locked artifact version allocation with no ready-state marker in
`crates/common/src/repositories/artifact.rs`. The separate-subsystem decision
is therefore well-justified.

The review surfaced four concrete gaps and two intent-level refinements.
Alternate solutions are recorded here rather than silently editing the
sections above, so the original design intent stays visible alongside the
correction.

### Gap 1: Supervisor maintenance integration is under-specified

**Issue (medium).** Section 9 says to add cache cleanup "consistent with other
maintenance loops," but the supervisor today runs a *single* retention cycle
guarded by *one* advisory lock key loaded from the `retention` config row and
reloaded each cycle (`crates/supervisor/src/main.rs`,
`crates/common/src/config.rs`). There is no framework of multiple independent
loops to be consistent with, and the design does not say how the cache
cleanup obtains its own leadership or cadence.

**Alternate solution.** Choose explicitly between two integration shapes and
record the choice:

- *Preferred:* run cache cleanup as a distinct step **inside** the existing
  supervisor retention cycle, reusing the current advisory lock and cycle
  cadence, but with its own DB-persisted `cache_retention` config sub-object
  (batch size, minimum availability window, staging-expiry interval) reloaded
  each cycle like the existing retention settings. This avoids a second
  leader election and keeps one maintenance heartbeat.
- *Only if cache cleanup must run at a different cadence:* add a **new,
  separately named** advisory lock key (never reuse the retention key) plus an
  independent interval field, and document that the two loops may hold
  leadership independently.

In both cases, cache cleanup stays in repository methods called by the
supervisor, never ad hoc SQL, matching the existing retention pattern.

**Implemented choice:** the cache step runs inside the existing locked
retention cycle. Its configuration is persisted as
`runtime_retention_config.cache_retention`, exposed through the retention API,
and reloaded with the enclosing retention configuration every cycle.

### Gap 2: SQLite export needs the atomic finalize contract, not a new storage path

**Issue (medium, optional feature).** `artifact_version` already supports three
storage modes (`crates/api/src/routes/artifacts.rs`,
`migrations/20250101000007_supporting_systems.sql`): structured JSON in
`content_json`, DB-stored binary in `content BYTEA` via the multipart
`POST /versions/upload` path (capped at `MAX_FILE_SIZE = 50 * 1024 * 1024`),
and **filesystem-backed content via `file_path`** through
`POST /versions/file`, which writes to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on
the shared artifact volume with no 50 MB cap. A large SQLite export therefore
uses the file-backed mode and does **not** need to go into a PostgreSQL
`BYTEA` column; the 50 MB limit applies only to the multipart DB-stored path,
which is the wrong path for this feature. The earlier claim that a large-file
artifact path "does not exist" was inaccurate.

The real, remaining gap is atomicity, not storage location. The file-backed
path allocates the version row **first** and expects the caller to write the
file to disk **after** receiving the response (see the `create_version_file`
docstring). There is no ready-state marker, so a reader can discover a version
whose file is still being written or was abandoned. This is exactly the
atomic-publication gap already described in the "SQLite Artifact Alternative"
section.

**Alternate solution.** State the single real dependency in Section 10:

1. Use the existing file-backed (`file_path`) artifact mode for the export;
   do not use the multipart/`BYTEA` path and do not store the database body in
   PostgreSQL.
2. The only prerequisite enhancement is the staging/ready finalize contract:
   allocate a staging version, write and validate the file, then atomically
   mark it ready (or publish a separate manifest pointer). List/latest/download
   must exclude versions that have not been marked ready, so a partially
   written or abandoned export is never discoverable.

No new large-file transport is required; the missing piece is the ready-state
marker so that "allocate version, then write file" becomes an atomic
publication rather than a discoverable partial write.

### Gap 3: Execution-token TTL is coupled to the action timeout

**Issue (low–medium).** Execution tokens are minted with a TTL of
`execution_timeout + 60s` (`crates/worker/src/executor.rs`; default action
timeout 360s). Because authorization is re-evaluated on every page and cursor
expiry is bounded by token expiry, an **action** performing a full
page-by-page 200,000-record scan will have its pinned traversal aborted
mid-scan if the action's configured timeout is shorter than the traversal
time. There is no mid-execution token renewal, so the design's "cursor
expiration should not exceed the current token expiration" rule silently caps
a long scan at the action timeout.

**Alternate solution.** Document the coupling and the mitigations in Sections 3
and 8:

- Full-scan actions must configure an `execution_timeout` that exceeds the
  expected traversal time; the SDK consumer should surface the token
  expiration up front so an author can size the timeout.
- For large datasets prefer the bounded multi-ID lookup or the single-response
  NDJSON scan (a server-side stream under one request) over a client-driven
  page-by-page loop, so the whole read completes within one token lifetime.
- Explicitly state that there is no execution-token refresh; a snapshot whose
  cursor outlives the token is expected to fail closed, and the client must
  not attempt to re-mint a token mid-execution.

### Gap 4: "standard execution access is read-only" is misleading

**Issue (low).** The Web Application section says "standard execution access is
read-only." As a statement about the *current* model this is inaccurate: the
existing `standard` grant gives Artifacts `Create`/`Update`/`Delete` and Keys
`Read`+`Decrypt` (`crates/api/src/authz.rs`). The design's *intent* — that
cache `standard` access be strictly read-only — is a deliberate and correct
divergence, but the wording implies `standard` is already read-only in
general.

**Alternate solution.** Reword to scope the claim to caches, e.g. "cache
`standard` access is read-only (unlike artifact `standard` access, which
grants writes); refresh, upload, sealing, promotion, and deletion require
explicit cache write grants." No design change is needed, only precise
phrasing so an implementer does not assume `standard` is uniformly read-only.

### Refinement A: State the minimal correct core explicitly

**Observation.** 200,000 JSONB rows is small for PostgreSQL; a single indexed
`(namespace_id, generation_id, external_id COLLATE "C")` table serves point
lookup and keyset scan directly. The initial scope still bundles roughly a
dozen mechanisms. Each is individually justified above (for example, the
`ready` seal genuinely closes the seal-versus-write race), but the document
does not distinguish the irreducible core from the hardening layers.

**Alternate framing.** Add a short "minimal correct core" statement to
*Initial Scope*: immutable generations plus a single-transaction
active-generation pointer swap already deliver snapshot coherence on their
own. Everything else — the `ready` intermediate state, signed/versioned
cursors, per-chunk digests, multi-tier quotas, and minimum-availability
retention windows — should be justified as an increment against that baseline.
This lets reviewers right-size the first delivery without weakening any
individual argument, and makes it clear which mechanisms could be phased if
schedule pressure appears (they should not be dropped, but the ordering
becomes explicit).

### Refinement B: Define tombstone-versus-in-flight-ingestion behavior

**Observation.** The design covers tombstone-and-batched-cleanup for namespace
and owner deletion, but not the race where a namespace (or its owner) is
tombstoned while a staging generation is actively receiving chunk uploads.

**Alternate solution.** Specify in Sections 5 and 9 that a tombstoned namespace
rejects further chunk uploads, seals, and promotions with a specific
`namespace_deleted` error, that any in-flight staging generation is marked
`failed` and swept by the same batched-cleanup path, and that the owner
restrict-delete continues to block hard owner deletion until cache cleanup
drains. This closes the one concurrency edge case not already addressed.

## Related Documentation

- [Secret Management API](api/api-secrets.md)
- [Secrets Management in the Worker Service](authentication/secrets-management.md)
- [Authorization Model](permissions/permissions-high-level.md)
- [Worker Service Architecture](architecture/worker-service.md)
- [Sensor Service Architecture](architecture/sensor-service.md)
- [Supervisor Service](deployment/supervisor.md)
- [File-Based Artifact Storage Plan](plans/file-based-artifact-storage.md)
- [CLI Reference](cli/cli.md)
- [Web UI Architecture](architecture/web-ui-architecture.md)
