# Plan: pack index refresh policies and Git-ref controls

**Status:** proposed implementation specification; not implemented.

This document records the requested plan only. API routes, fields, YAML upload,
and index format extensions described as proposed below do not exist yet.
Field tables are normative proposed schemas: together with the validation and
cross-field rules they define the complete documents for this feature. Implementation
must publish equivalent OpenAPI schemas and a downloadable JSON Schema usable
by YAML editors. These are configuration/API schemas, not Attune action
`param_schema`, `out_schema`, or `conf_schema` documents.

## 1. Goals and agreed direction

- Administrators independently control how each index is refreshed.
- Public indices can remain unchanged until an explicit administrator refresh,
  analogous to `apt update`.
- Internal indices can refresh automatically during installation, **after a
  per-index freshness interval expires**. This interval-based behavior was
  explicitly selected; refreshing on every install is not the selected design.
- Lazy indices refresh once when a specifically requested pack/version is absent.
- Each index controls which Git refs callers may request.
- The standard public index defaults to manual refresh and numeric three-component
  release selectors, such as `1.0.2` or `12.10.3`.
- Saved snapshots survive API restarts and are shared across API replicas.
- API, CLI, web UI, and uploaded YAML use the same validation and semantics.

### Scope boundaries

Automatic refresh is request-driven, not scheduled polling, a webhook service, or
an automatic upgrade of installed packs. Refreshing metadata never installs a pack.
This work does not grant private-network access, forward index credentials to pack
repositories, or weaken existing direct-remote-install integrity controls.

Details not already agreed with the owner are **proposed defaults**, not claims
of existing behavior. Section 13 lists implementation decisions requiring review.

## 2. Existing implementation and constraints

Relevant current code:

- `crates/common/src/config.rs`: `RegistryIndexConfig`, `PackRegistryConfig`.
- `crates/common/src/models.rs`: managed index model.
- `crates/common/src/repositories/pack_registry_index.rs`: managed index CRUD.
- `crates/common/src/pack_registry/client.rs`: fetching, validation, transient cache.
- `crates/common/src/pack_registry/mod.rs`: remote index document and source types.
- `crates/api/src/dto/pack.rs`: management, browsing, installation DTOs.
- `crates/api/src/routes/packs.rs`: index routes and `resolve_registry_request()`.
- `docs/packs/pack-registry-spec.md`: existing remote catalog and installation spec.

The existing cache is in memory, uses a global TTL, and belongs to a
`RegistryClient`; API paths construct new clients. It cannot provide durable
manual-only semantics. Existing managed indices use `position`; static YAML
indices use `priority`. Existing catalog entries advertise one latest `version`;
installation rejects explicitly requested versions different from that value.

Existing production catalog documentation requires immutable Git source SHAs and
source-specific checksums. Therefore **a release selector and its concrete Git
source are not interchangeable**. Allowing `1.2.3` but denying caller-supplied
commit refs must still permit release `1.2.3` to install from its indexed SHA.

### Tracking URLs versus installed snapshots

The current standard-index migration
`migrations/20260822000003_standard_pack_index_v040.sql` selects a commit-pinned
`raw.githubusercontent.com` URL. Re-fetching it cannot discover new releases.

In this plan, configuration `url` means the **tracking URL**. The standard seed
must use the maintained release channel (currently proposed as
`https://raw.githubusercontent.com/attune-system/index/main/index.json`), after
confirming that upstream treats that channel as release metadata. Manual mode
still prevents fetching without administrator action. Save each fetched snapshot
with its content digest and revision; never confuse the moving URL with immutable
installation provenance. An explicitly configured commit-pinned tracking URL is
valid, but the UI/preflight should warn that refresh cannot discover newer data.
Only recognized standard seed URLs are migrated automatically; do not rewrite
operator-customized URLs. Refresh does not imply signature verification: retain
HTTPS/source-checksum controls and address signed catalog trust separately.

## 3. Policy semantics

### 3.1 Refresh modes

| Situation | `manual` | `automatic` | `lazy` |
|---|---|---|---|
| Installation, no saved snapshot | Fail `index_not_initialized` | Fetch once | Fetch once |
| Installation, saved snapshot fresh | Use snapshot | Use snapshot | Use snapshot |
| Installation, snapshot age reaches interval | Use snapshot | Refresh before resolution | Use snapshot |
| Explicit pack/version missing | Fail without fetching | Resolve after ordinary freshness check; no extra miss refresh | Refresh once, retry, then fail if still absent |
| Unversioned / `latest` request, pack exists | Use saved latest | Use latest from freshness-checked snapshot | Use saved latest; do not refresh just to discover newer releases |
| Requested pack itself absent | Fail without fetching | Ordinary freshness check only | Refresh once and retry, including an unversioned request |
| Browse/search/detail metadata request | Saved snapshot only | Saved snapshot only | Saved snapshot only |
| Explicit administrator refresh | Fetch regardless of age | Fetch regardless of age | Fetch regardless of age |

Freshness is measured from the last successful fetch or successful conditional
revalidation, not from the remote document's `last_updated`. A snapshot is stale
when `now >= last_successful_refresh_at + refresh_interval_seconds`. Unexpected
backward clock movement makes automatic freshness conservative (stale).

A lazy request has at most one network-refresh attempt per candidate index.
Fetching an initially absent snapshot consumes that attempt. A successful
refresh does not imply that an arbitrary requested version exists. The mode table
is subject to section 3.1.1: a recent confirmed miss may reuse its negative result,
and throttled attempts return retry guidance rather than bypassing the bound.

Failure of a required refresh fails the install; do not silently fall back to
stale metadata. Preserve the last good snapshot for manual use and diagnosis.
This deliberately makes automatic installs dependent on upstream availability
after expiry, even if an exact requested release is cached. UI, API errors, and
operator documentation must state that consequence. An administrator can explicitly
switch to manual mode to use the saved snapshot, with an audited policy change.

A bounded stale-on-error option is deferred, not silently enabled: if added later,
it must be opt-in, limited by snapshot age, and restricted to already-cached exact
indexed releases with verified source metadata. It must never apply to `latest`,
unversioned requests, missing releases, unindexed refs, source-identity changes,
or authorization/policy denials. Its eventual schema requires separate review.
The initial schema accepts no stale-on-error field.

### 3.1.1 Lazy miss protection and failure backoff

`lazy_refresh_min_interval_seconds` (default 30) bounds miss-triggered refresh
starts per index, shared across API replicas. It applies only in lazy mode and
is independent of automatic freshness. The first fetch of an uninitialized index
is permitted; subsequent misses respect this floor and failure backoff.

Cache confirmed negative lookups for 30 seconds from a successful refresh/revalidation,
keyed by index/source identity, snapshot revision, pack, and exact selector. Bound
entry count with eviction. A repeat of that confirmed miss may return not-found
without another fetch during its negative-cache lifetime. Unknown selectors during
the cooldown return `index_refresh_throttled` and `Retry-After`, not a definitive
not-found. Clear negative entries on successful refresh (including HTTP 304),
source invalidation, and relevant policy changes before recording new misses.

Failed automatic/lazy fetches use shared exponential backoff starting at 5 seconds,
capped at 300 seconds, with jitter of up to 20% added to the delay. Backoff does not
extend snapshot freshness or authorize stale installs. A successful fetch resets
backoff. Explicit administrator jobs bypass miss cooldown/negative caches, but
remain subject to administrator rate limits, queue bounds, and upstream
`Retry-After`. Effective `next_refresh_allowed_at` reports the applicable delay.
Coalesce concurrent requests for the same identity and revision; each lazy
installation still performs at most one refresh/retry cycle.

### 3.2 Disabled indices and precedence

- `enabled: false` excludes the index from installation and ordinary browsing.
- Explicit refresh of a disabled index is permitted for administrator preparation.
- Refresh-all excludes disabled indices unless `include_disabled: true`.
- `pack_registry.enabled: false` preserves existing global remote-operation
  restrictions; refreshing remotely is disabled too. Configuration CRUD remains
  available.
- `registry_id` pins resolution to a managed index and rejects a disabled index.
- Keep existing managed/static ordering and URL deduplication semantics. Pin them
  in tests before replacing the resolver; do not invent a second ordering model.
- The first eligible index containing a pack owns resolution of that pack. Missing
  versions and policy rejection must not fall through to a lower-priority index
  containing the same pack. This prevents a dependency-confusion bypass.
- Lazy refresh on a pack miss happens before advancing to the next index.
- Failure to inspect a higher-priority index is an error, not permission to skip it.

### 3.3 Git selector policy

**Delivery stages:** Stage A implements snapshot refresh and indexed-release
selector checks. Stage B separately introduces unindexed Git refs after the
integrity review and dedicated tests. Stage A accepts only
`allow_unindexed_git_refs: false`; sending true returns
`feature_not_available`, rather than silently ignoring it. The true-valued
examples below are explicitly Stage B examples, not Stage A configuration.

Proposed policy has two independent controls:

1. `allowed_git_ref_pattern`: a full-string allowlist for the external selector.
2. `allow_unindexed_git_refs`: whether an explicit ref may be resolved from the
   catalog-declared repository without a matching indexed release.

The second setting is necessary because syntactic acceptance alone must not
silently waive indexed release metadata/checksums.

- For an indexed release, check its selected release version against the pattern;
  the indexed concrete SHA is not a caller-requested commit and is not rejected
  merely because it is a hash.
- For an explicit `ref_spec`, check the supplied Git ref against the pattern and
  require `allow_unindexed_git_refs: true`.
- For `latest` or an omitted version, resolve the latest release first, then check
  that release version. Do not test the literal word `latest` against the pattern.
- Reject disallowed explicit refs before a network refresh or Git operation.
- The pattern applies to Git installations, including Git-to-archive fallback of
  the same selected release. An archive-only pack has no Git ref to authorize.
- Regex matching alone does not distinguish a branch called `1.2.3` from a tag
  called `1.2.3`. Tag-only enforcement is not promised by this feature.
- Resolve accepted mutable Git refs to an immutable commit before fetching/installing;
  pin that commit throughout installation and record both requested and resolved refs.
- Index policy cannot authorize arbitrary caller-supplied repository URLs. Unindexed
  refs use a dedicated, catalog-declared repository field (section 8).
- Direct URL installs remain subject to the existing separate direct-install policy;
  they must not be represented as policy-verified index installs.

## 4. Common schema and validation rules

All proposed write documents reject unknown keys, duplicate mapping keys, explicit
nulls unless listed as nullable, and wrong primitive types. YAML parsing rejects
custom tags, multiple documents, non-string map keys, and non-JSON-compatible
values. Bound parser input and expansion before deserializing.

| Type | Definition |
|---|---|
| `IndexId` | Positive signed 64-bit integer; API JSON integer, never floating point |
| `Position` | Integer `0..2147483647`; lower sorts earlier |
| `Interval` | Integer seconds `1..604800` (one second through seven days) |
| `Timestamp` | RFC 3339 UTC string |
| `Mode` | Exactly `manual`, `automatic`, or `lazy`, case-sensitive |
| `Headers` | Object mapping HTTP header names to string values; default `{}` |
| `Pattern` | UTF-8 string, 1–4096 bytes; Rust `regex` syntax; full-string semantics |
| `IndexUrl` | Absolute HTTPS tracking URL with host; no userinfo, fragment, or query string |
| `StaticKey` | 1–64 ASCII characters matching `[a-z][a-z0-9_-]{0,63}`; stable, unique within the instance |
| `OperationId` | Positive signed 64-bit integer for a durable refresh operation |

Patterns are compiled on create/update/import/startup, with bounded compiled size.
Unsupported syntax such as backreferences is rejected. Full matching must be
implemented with absolute boundaries around the configured expression, not by
trusting caller-supplied anchors. No trimming or case normalization of Git refs.

Header names/values must pass existing registry header validation. Encrypt managed
header values at rest and always redact them on read. Reject the literal
`[REDACTED]` as an imported/updated credential value to prevent accidental export
round trips from replacing real credentials. Omitted update headers preserve
credentials; `{}` clears them. No environment-variable interpolation in YAML.

URLs remain subject to existing outbound host/network approval, DNS/redirect,
HTTPS, timeout, and bounded-response controls. Uploading configuration does not
add hosts to those approvals. This proposal keeps managed and uploaded URLs HTTPS
only; static HTTP behavior remains governed by the existing global `allow_http`.

## 5. API configuration document schemas

All paths below have prefix `/api/v1`. Existing routes are retained unless noted.
Protected routes require `RequireAuth`; writes and refreshes require existing
global index-administration authorization, including its constraint checks.

### 5.1 `CreateIndex` — complete request body

`POST /pack-indices`, `Content-Type: application/json`.

| Field | Type | Required / default | Meaning |
|---|---|---|---|
| `name` | string or null | optional, null | Display name; nonempty after trimming if provided; max 255 characters |
| `url` | `IndexUrl` | required | Tracking URL used for future refreshes, not snapshot identity |
| `position` | `Position` | optional, append | Search order; ties retain existing ID ordering |
| `enabled` | boolean | optional, true | Eligible for resolution |
| `headers` | `Headers` | optional, `{}` | Credentials for index fetch only |
| `update_mode` | `Mode` | optional, `manual` | Refresh policy |
| `refresh_interval_seconds` | `Interval` or null | conditional | Automatic: omitted means 3600; explicit null invalid. Other modes: omitted/null only |
| `lazy_refresh_min_interval_seconds` | `Interval` or null | conditional | Lazy: omitted means 30; explicit null invalid. Other modes: omitted/null only |
| `allowed_git_ref_pattern` | `Pattern` | optional, `^[0-9]+\.[0-9]+\.[0-9]+$` | Accepted release versions / explicit Git refs |
| `allow_unindexed_git_refs` | boolean | optional, false | Opt into catalog-repository Git-ref installs without an indexed release checksum |

Create persists configuration only: it does not fetch. The standard public seed
uses these defaults. Private indices require explicit opt-in to automatic/lazy
mode or a broader ref pattern. The following example opts into the Stage B
unindexed-ref feature; use false for Stage A.

```json
{
  "name": "Platform packs",
  "url": "https://packs.example.internal/index.json",
  "position": 0,
  "enabled": true,
  "headers": {},
  "update_mode": "automatic",
  "refresh_interval_seconds": 300,
  "allowed_git_ref_pattern": "(?:[0-9]+\\.[0-9]+\\.[0-9]+|main|[0-9a-f]{40})",
  "allow_unindexed_git_refs": true
}
```

### 5.2 `UpdateIndex` — complete partial-update body

Existing `PUT /pack-indices/{id}` retains partial-update semantics. Every field
from `CreateIndex` is optional, with the following differences:

| Field / case | Update meaning |
|---|---|
| Any omitted field | Preserve stored value, subject to mode transition rules below |
| `name: null` | Clear display name |
| `headers: {}` | Clear all headers; object otherwise replaces the complete header map |
| `headers: null` | Reject; use `{}` to clear |
| `refresh_interval_seconds: null` | Clear interval only if resulting mode is not automatic |
| `lazy_refresh_min_interval_seconds: null` | Clear cooldown only if resulting mode is not lazy |
| `invalidate_snapshot` | Optional update-only boolean, default false; true explicitly invalidates saved contents atomically with configuration changes; null rejected |
| `url`, `position`, `enabled`, `update_mode`, pattern, unindexed flag as null | Reject |
| Empty object `{}` | Valid no-op; no refresh or snapshot invalidation |

Mode transitions are atomic:
- Entering automatic with no supplied/stored interval uses 3600 seconds.
- Remaining automatic with omitted interval preserves it.
- Entering manual/lazy with omitted interval clears it.
- Explicit non-null interval with resulting manual/lazy mode is rejected.
- Entering lazy with no supplied/stored cooldown uses 30 seconds; remaining lazy
  preserves it when omitted. Leaving lazy clears it when omitted. A non-null
  cooldown in any other resulting mode is rejected.
- No configuration update implicitly fetches metadata.

Separate configuration, source, and credential revisions:
- Every effective configuration mutation increments `configuration_revision`.
- URL changes or `invalidate_snapshot: true` increment `source_revision` and
  invalidate snapshot eligibility. Old contents are not a fallback for that source.
- Header changes increment `credential_revision` and fence in-flight fetches,
  but preserve the saved snapshot and its original freshness timestamp. Reset
  credential-specific failed-fetch backoff; do not extend snapshot freshness.
- When new credentials change tenant or metadata visibility rather than merely
  rotate a token, administrators **must** send `invalidate_snapshot: true` in
  the same update. The UI presents this choice explicitly; the server cannot
  infer a credential's tenant from opaque secret bytes.
- Policy edits reauthorize subsequent resolutions without fetching. Name/position/
  enabled edits do not discard contents. `invalidate_snapshot` is an operation
  flag, not a stored policy or a response field.

Provide the equivalent invalidation-only API for file-owned indices in section 6.
Changing credentials while a manual index is temporarily offline must not by
itself interrupt installs from an otherwise valid saved snapshot.

### 5.3 `Index` — complete response object

`GET /pack-indices` returns `ApiResponse<Index[]>`; create returns HTTP 201 and
`ApiResponse<Index>`; update returns HTTP 200 and `ApiResponse<Index>`. Proposed
`GET /pack-indices/{id}` returns HTTP 200 and the same object. Existing deletion
retains its existing success response.

Every field below is present, including explicitly nullable fields:

| Field | Type | Meaning |
|---|---|---|
| `id` | `IndexId` | Managed index identity |
| `name` | string or null | Display name |
| `url` | string | Normalized URL |
| `position` | `Position` | Managed ordering |
| `enabled` | boolean | Enabled state |
| `is_standard` | boolean | Read-only standard-index marker |
| `headers` | object of strings | Header names retained; every value exactly `[REDACTED]` |
| `update_mode` | `Mode` | Effective configured mode |
| `refresh_interval_seconds` | `Interval` or null | Non-null only in automatic mode |
| `lazy_refresh_min_interval_seconds` | `Interval` or null | Non-null only in lazy mode |
| `allowed_git_ref_pattern` | string | Configured pattern |
| `allow_unindexed_git_refs` | boolean | Explicit unindexed opt-in |
| `configuration_revision` | positive i64 | Changes on effective configuration mutations |
| `source_revision` | positive i64 | Fences URL changes and explicit snapshot invalidation |
| `credential_revision` | positive i64 | Fences credential rotations without discarding saved contents |
| `next_refresh_allowed_at` | `Timestamp` or null | Earliest automatic/lazy attempt after cooldown/backoff; null means no active delay |
| `snapshot_digest` | string or null | `sha256:<64 lowercase hex>` of the canonical validated saved document, using one documented canonical JSON encoding |
| `snapshot_revision` | positive i64 or null | Current eligible snapshot revision |
| `snapshot_status` | `missing`, `ready`, or `invalidated` | Eligible metadata availability, not upstream reachability |
| `snapshot_fetched_at` | `Timestamp` or null | When current contents were fetched |
| `last_successful_refresh_at` | `Timestamp` or null | Includes successful conditional revalidation |
| `last_refresh_attempt_at` | `Timestamp` or null | Most recent actual fetch attempt |
| `last_refresh_error` | `RefreshError` or null | Most recent attempt failure; cleared on success |
| `refresh_in_progress` | boolean | Advisory current refresh state |
| `created` | `Timestamp` | Creation time |
| `updated` | `Timestamp` | Configuration update time; fetches do not change this |

`RefreshError` contains exactly `code: string`, `message: string`, and
`occurred_at: Timestamp`. Codes come from section 10. Messages are sanitized;
never include response bodies, credentials, or authorization headers.

`ApiResponse<T>` remains `{ "data": T, "message"?: string }`.
Response-only fields cannot be submitted in create/update/import documents.

## 6. Refresh API schemas

### 6.1 Durable refresh submission

**New:** `POST /pack-indices/{id}/refresh` for one managed index and
`POST /pack-indices/static/{key}/refresh` for one file-owned index. No body/query
parameters. Fetch regardless of snapshot age/mode, subject to section 3.1.1 limits.
A concurrent fetch can satisfy the request if it starts after operation submission
and against the same captured revisions; otherwise coalesce callers into a bounded
subsequent attempt. Do not start one additional fetch per waiting administrator.

Both routes return HTTP **202**, `ApiResponse<RefreshOperation>`, and a `Location`
header pointing to `/api/v1/pack-indices/refresh-operations/{operation_id}`.
The job is durably recorded before returning; client disconnection does not cancel
it. Single-index and batch refresh use the same operation protocol.

**New:** `POST /pack-indices/refresh` submits a batch with JSON body:

| Field | Type | Default | Behavior |
|---|---|---|---|
| `include_disabled` | boolean | false | Include disabled managed/static indices |

No other fields; absent body means `{}`. Capture the effective deduplicated index
set and order at submission; later additions are not silently included. An empty
set creates an immediately succeeded operation. Shadowed static entries are
excluded from the batch but remain inspectable and individually refreshable.

All submission routes accept optional `Idempotency-Key`: 1–128 printable ASCII
characters, scoped to authenticated principal and route. Persist the key, request
fingerprint, and operation together for seven days. Identical retries return the
same operation (202 while pending, 200 when terminal); different payload under
the same key returns 409 `idempotency_conflict`. Reuse after retention expiry may
create new work. CLI/UI generate a key before submission and reuse it on retries.

### 6.2 Operation status and complete result schema

**New:** `GET /pack-indices/refresh-operations/{operation_id}` returns HTTP 200,
`ApiResponse<RefreshOperation>`. Inspection requires global index-administration
authorization. Polling intervals should honor `Retry-After` on nonterminal status
responses. Retain operations/idempotency records at least seven days after
completion; never expire active work. An absent/expired ID returns 404.

`RefreshOperation` contains exactly:

| Field | Type | Meaning |
|---|---|---|
| `id` | `OperationId` | Durable operation identity |
| `state` | `queued`, `running`, `succeeded`, `partially_failed`, or `failed` | Aggregate state; mixed success/failure is partially_failed |
| `created_at` | `Timestamp` | Accepted time |
| `started_at` | `Timestamp` or null | First item start |
| `completed_at` | `Timestamp` or null | Non-null exactly when terminal |
| `expires_at` | `Timestamp` or null | Retention deadline; null until terminal |
| `items` | `RefreshItem[]` | Captured target order; progress and result per target |

`RefreshItem` contains exactly:

| Field | Type | Meaning |
|---|---|---|
| `registry_id` | `IndexId` or null | Managed identity |
| `static_key` | `StaticKey` or null | File-owned identity; exactly one of ID/key is non-null |
| `registry_url` | string | Captured credential-free tracking URL |
| `configuration_revision` | positive i64 | Captured configuration revision |
| `source_revision` | positive i64 | Captured source revision |
| `credential_revision` | positive i64 | Captured credential revision; never credential values |
| `state` | `queued`, `running`, `succeeded`, or `failed` | Item progress |
| `started_at` | `Timestamp` or null | Start time |
| `completed_at` | `Timestamp` or null | Completion time |
| `outcome` | `updated`, `unchanged`, `failed`, or null | Null until terminal |
| `snapshot_revision` | positive i64 or null | Eligible revision at completion; null before completion or without a snapshot |
| `last_successful_refresh_at` | `Timestamp` or null | Success time observed at completion; null before completion or no success |
| `error` | `RefreshError` or null | Non-null exactly when failed |

A refresh worker owned by the API service claims durable items using bounded
DB leases and fencing tokens, obtains credentials at execution time, and performs
HTTP outside DB transactions. Bound instance-wide concurrency and queue length.
Recover after API restarts/worker loss; publish only against matching captured
revisions and current lease ownership. If configuration changes or the index is
deleted before execution/publication, terminate that item with a clear conflict
error rather than refreshing a different target. This includes an enablement edit
that changes the captured configuration revision; a fresh job may explicitly
refresh the now-disabled index. A global remote operation shutdown prevents
remaining fetches. Check it again before each start.

Each item gets one bounded network attempt; lease recovery may repeat an interrupted
GET, but must never double-publish. Do not automatically retry completed failures
indefinitely. Operators can submit another job after the reported backoff. Jobs
share the same per-index coordination as automatic/lazy installs; those installs
wait only within their request budget, then return a retryable result rather than
holding HTTP requests open indefinitely.

Pre-submission authorization, validation, rate/queue bounds, and identity failures
use ordinary non-2xx responses. Once accepted, upstream failures appear in item
results, not as HTTP 502 on a successful status GET. The existing good snapshot
remains eligible unless its source identity was invalidated.

### 6.3 Static inspection and explicit invalidation

**New:** `GET /pack-indices/static` returns `ApiResponse<StaticIndex[]>`;
`GET /pack-indices/static/{key}` returns `ApiResponse<StaticIndex>`. Both require
global index-administration authorization. `StaticIndex` has all `Index` fields
from section 5.3 **except** `id`, `position`, and `is_standard`, plus:
- `key: StaticKey` (required).
- `priority: u32` (required).
- `ownership: "static"` (required).
- `effective: boolean` (required; false when shadowed by existing dedup rules).
- `configuration_state: "ready" | "conflict"` (required).

Its `created`/`updated` represent first registration and last accepted definition
change; credentials stay redacted. Inspection of conflicts shows the last accepted
definition, not arbitrary competing replica values. Conflict disables refresh and
resolution for that static identity; do not fall through to lower-priority packs.

**New:** `POST /pack-indices/{id}/invalidate-snapshot` and
`POST /pack-indices/static/{key}/invalidate-snapshot`, no body/query parameters.
Require global index-administration authorization. Atomically increment the source
revision, invalidate saved contents/negative entries, and fence in-flight jobs;
return HTTP 200 with the corresponding `Index` or `StaticIndex` in `ApiResponse`.
Idempotent effect: already-invalidated contents stay invalidated, although the
revision may advance. No remote fetch occurs. Audit invalidation separately.
For a static credential scope change, invalidate **before** deploying the new
credentials; token-only rotation need not invalidate. The static rollout protocol
in section 7.2 fences operations while definitions disagree.

### 6.4 Browse semantics

Retain existing `/pack-indices/packs` and `/pack-indices/packs/{ref}` routes and
filters. They read eligible saved snapshots only. Missing snapshots must be
visible as initialization warnings, not silently represented as an empty index.
Extend browse responses with per-index snapshot status metadata; update both
OpenAPI and UI consumers together. Do not silently change existing item shapes.
The precise browse envelope change is a separate compatibility review item in
section 13, not a new write-document format.

## 7. Uploadable YAML and static service configuration

### 7.1 Configuration upload document — complete schema

**New:** `POST /pack-indices/import`, accepting `application/yaml` (UTF-8 raw
body), or `application/json` with the exact same data model. No multipart wrapper.
This imports index *configuration*, not a remote catalog or pack archive.
Proposed limits: 1 MiB body, 100 entries per request.

Root object:

| Field | Type | Required / default |
|---|---|---|
| `api_version` | string, exactly `attune/v1` | required |
| `kind` | string, exactly `PackIndexConfiguration` | required |
| `indices` | array of `ImportIndex`, 1–100 items | required |

Each `ImportIndex` contains all fields of `CreateIndex` with identical validation
and defaults, plus optional `id: IndexId`:

- Without `id`: create; `url` required; create defaults apply.
- With `id`: update that managed index; other fields have `UpdateIndex` omission
  semantics and need not include `url`. The update-only `invalidate_snapshot`
  flag is allowed here, but rejected on entries without `id`.
- Duplicate IDs or normalized URLs in the resulting configuration are rejected.
- An unknown ID is an error; never implicitly create or match by display name.
- Unmentioned existing indices are retained. Import never deletes indices.
- `is_standard`, timestamps, status, revisions, cached contents, and global
  network/security settings are not writable fields.
- Validate the entire document before a single transaction applies changes.
  Any error rolls back all changes. Index fetches never happen in the transaction.
- Import never triggers refresh, including for automatic indices. The next install
  or a separate explicit refresh performs the fetch.
- Read responses with redacted credentials are not directly re-importable exports.
  A future exporter must omit headers unless explicitly supplied from a secret store.

Complete example (the third entry requires Stage B; use
`allow_unindexed_git_refs: false` during Stage A):

```yaml
api_version: attune/v1
kind: PackIndexConfiguration
indices:
  - name: Public releases
    url: https://registry.example.com/index.json
    position: 10
    enabled: true
    headers: {}
    update_mode: manual
    refresh_interval_seconds: null
    allowed_git_ref_pattern: '^[0-9]+\.[0-9]+\.[0-9]+$'
    allow_unindexed_git_refs: false

  - name: Platform releases
    url: https://packs.example.internal/index.json
    position: 0
    enabled: true
    headers:
      Authorization: 'Bearer example-placeholder-not-a-real-secret'
    update_mode: automatic
    refresh_interval_seconds: 300
    allowed_git_ref_pattern: '^[0-9]+\.[0-9]+\.[0-9]+$'
    allow_unindexed_git_refs: false

  - name: Team development
    url: https://development.example.internal/index.json
    position: 1
    enabled: true
    headers: {}
    update_mode: lazy
    refresh_interval_seconds: null
    lazy_refresh_min_interval_seconds: 30
    allowed_git_ref_pattern: '(?:[0-9]+\.[0-9]+\.[0-9]+|main|feature/[A-Za-z0-9._/-]+|[0-9a-f]{40})'
    allow_unindexed_git_refs: true
```

Partial update example (preserves URL, credentials, and ref controls):

```yaml
api_version: attune/v1
kind: PackIndexConfiguration
indices:
  - id: 42
    update_mode: automatic
    refresh_interval_seconds: 600
```

Import success: HTTP 200, `ApiResponse<ImportResult>`.
`ImportResult` contains exactly `results: ImportItemResult[]`, in document order.
Each result contains `document_index: integer` (zero-based), `operation: created |
updated | unchanged`, and `index: Index` (section 5.3). Errors identify field paths
such as `indices[1].refresh_interval_seconds` without repeating secret values.

### 7.2 Static service YAML — complete per-index object

Static entries under `pack_registry.indices` use these fields:

| Field | Type / default | Relation to managed API |
|---|---|---|
| `key` | required `StaticKey` | Stable file-owned identity, independent of URL and display name |
| `url` | required tracking URL | Existing static URL validation and outbound policy; no file URLs introduced |
| `priority` | unsigned 32-bit integer, default 100 | Existing static ordering; `position` is not accepted here |
| `enabled` | boolean, default true | Same behavior |
| `name` | string or null, default null | Same display validation |
| `headers` | `Headers`, default `{}` | Literal local secrets; not written back to configuration files |
| `update_mode` | `Mode`, default `manual` | Same behavior |
| `refresh_interval_seconds` | `Interval` or null | Same create rules |
| `lazy_refresh_min_interval_seconds` | `Interval` or null | Same create rules |
| `allowed_git_ref_pattern` | `Pattern`, default numeric triplet | Same behavior |
| `allow_unindexed_git_refs` | boolean, default false | Same behavior |

No managed `id`, import wrapper, or response/status fields are accepted in a static
entry. Static entries remain file-owned; management API cannot edit them. Reload
uses the existing service configuration lifecycle (do not promise hot reload).
Persist snapshots under the stable `key`, scoped to the instance, with separate
configuration/source/credential revisions. Changing URL invalidates contents;
rotating a token alone does not. Internal snapshot IDs are i64, not public managed
IDs. Keys cannot collide within a config; renaming a key creates a new identity
and requires explicit initialization. Removed entries are retired from the effective
set; retain referenced provenance according to retention policy.

Replica ownership protocol: maintain an accepted, normalized static-config manifest
and a keyed digest of credential values (never plaintext or an unkeyed secret hash).
At startup replicas compare their desired manifest with the accepted manifest.
Mismatch marks affected identities conflicted, blocks their resolution/refresh,
and emits a readiness diagnostic; no replica may automatically overwrite another's
accepted definition. Unaffected indices remain usable. Existing managed/static
ordering and shadowing remain explicit in inspection responses.

Provide operator command `attune pack index reconcile-static <config-file>` to
validate and accept a changed manifest through an administrator-only deployment
workflow; it does not convert static entries into API-editable indices. Publish
a deployment runbook to drain index work, reconcile once, roll out matching config
to all replicas, and resume after agreement. Missing keys, differing URLs/policies,
and credential digests are checked. Old replicas remain fenced until updated;
reconciliation must never clear conflict while active replicas still disagree.
The internal manifest-acceptance contract must be specified before implementing
this workflow; ordinary import endpoints cannot edit file-owned definitions.

```yaml
pack_registry:
  enabled: true
  indices:
    - key: internal-releases
      name: Internal releases
      url: https://packs.example.internal/index.json
      priority: 10
      enabled: true
      headers: {}
      update_mode: automatic
      refresh_interval_seconds: 300
      allowed_git_ref_pattern: '^[0-9]+\.[0-9]+\.[0-9]+$'
      allow_unindexed_git_refs: false
```

This is a fragment of the service config, not an upload document. Uploading it to
`/pack-indices/import` fails validation; use the explicit import wrapper instead.
Conversely, the import wrapper is not accepted in `config.*.yaml`.

### 7.3 Global `pack_registry` settings and interactions

Existing global fields remain service-owned and cannot be set by an index upload.
The following inventory describes all current fields and proposed interactions:

| Field | Existing default | Planned behavior |
|---|---|---|
| `enabled` | true | Global gate on remote operations |
| `indices` | `[]` | Static entries above, combined with managed entries as today |
| `cache_ttl` | 3600 | Removed from the new runtime contract; preflight uses legacy value only to propose explicit per-index settings |
| `cache_enabled` | true | Removed from the new runtime contract; preflight requires an explicit replacement for false; durable snapshots are always used |
| `timeout` | 120 seconds | Existing overall download timeout |
| `connect_timeout` | 10 seconds | Existing connection timeout |
| `verify_checksums` | true | Preserve source-specific verification; unindexed Git opt-in is separately disclosed |
| `allow_unverified_direct_remote_installs` | false | Direct URL policy only; does not grant unindexed index installs |
| `approved_public_hosts` | raw.githubusercontent.com, github.com, codeload.github.com | Existing host approvals; custom public index hosts require administrator approval |
| `approved_private_hosts` | `[]` | Existing explicit private-host approvals |
| `approved_private_cidrs` | `[]` | Existing private network approvals |
| `allow_http` | false | Existing transport control; does not relax managed/upload HTTPS requirement |
| `index_max_bytes` | 10485760 | Bound remote metadata responses and parsed snapshot size |
| `archive_max_bytes` | 104857600 | Existing archive limit |

Explicit legacy `cache_ttl`/`cache_enabled` keys fail new configuration validation
with preflight conversion guidance; absent keys are not treated as legacy defaults
at runtime. Do not silently reinterpret `cache_enabled: false` as TTL caching.
Section 12 specifies how to convert old deployments before enabling the new
resolver. Static credentials are literal; `${TOKEN}` is not expanded. Prefer
API/import credentials supplied securely by the caller.

## 8. Version catalogs and installation request schema

### 8.1 Proposed remote index v2 extension

Keep v1 readable by adapting its single entry into one indexed release. Publish a
v2 catalog schema before producers adopt historical releases; do not relax v1
unknown-field validation in place.

Proposed v2 envelope retains `registry_name`, `registry_url`, `last_updated`, and
`packs` from the existing catalog specification; `version` is exactly `"2.0"`.
Each pack retains existing descriptive fields, contents, and metadata from
`docs/packs/pack-registry-spec.md`, but replaces pack-level `version` and
`install_sources` with:

| Field | Type | Meaning |
|---|---|---|
| `latest_version` | nonempty string, required | Must exactly reference one member of `releases` |
| `releases` | nonempty array of `Release`, required | Unique version strings per pack |
| `git_repository` | approved HTTPS repository URL or null, optional | Explicit repository for unindexed ref requests; never use the homepage field |

`Release` fields are exactly:
- `version`: required nonempty string, max 255 characters.
- `install_sources`: required nonempty array of existing `InstallSource` objects;
  retain current Git/archive shapes and source-specific checksums.
- `dependencies`: optional existing `PackDependencies` object, scoped to the release.

Move dependency requirements to each release in v2; descriptive pack metadata is
not an authority for historical release dependencies. Continue to validate the
installed manifest identity/version against the selected indexed release.

This v2 extension is a proposal, not a complete replacement of the catalog spec;
implementation must publish the fully expanded v2 JSON Schema, including the
existing nested source/contents/dependency definitions, alongside the current v1
schema. The complete new *configuration* and *upload* documents are sections 5–7.

### 8.1.1 Historical catalog growth and artifact availability

Do not raise `index_max_bytes` indefinitely to accommodate release history. Stage A
supports bounded v1 catalogs; the v2 format must not ship as an unbounded flat
history. Before adopting v2, specify either bounded retained-history guarantees
with explicit unavailable-version errors, or a partitioned catalog. Preferred
partitioning is a small root snapshot with content-addressed per-pack release
manifests, bounded document size/count/aggregate fetch budget, and digest-pinned
links. Publish the complete schema and producer tests before rollout.

All lazy historical fetches must remain within one logical root generation; never
combine mutable root and release documents from different snapshots. Manual
operation must not fetch newer metadata through a side door: either materialize
required metadata during explicit refresh, or fetch only immutable documents
whose digests are already pinned by the saved root. Any on-demand immutable fetch
must be documented as network availability, not a refresh of saved metadata.
The first implementation uses materialized bounded snapshots; partitioned
on-demand access is a later separately specified protocol.

A retained metadata snapshot is **not** a guarantee that an archive or Git object
remains available upstream. Document that exact version pinning provides identity,
not artifact availability. Evaluate optional content-addressed artifact mirroring
for installations that need reproducibility/offline operation. A mirror must
verify source-specific checksums on ingestion and retrieval, enforce source/tenant
access controls, and define retention/GC for installed or pinned releases. Mirror
storage is out of Stage A scope and introduces no configuration fields here.
If an artifact disappears, fail with a specific source-unavailable error; never
silently substitute another release or downgrade checksum verification.

### 8.2 Installation API — complete request body

Retain existing `POST /packs/install` request fields, with explicit registry-ref
semantics:

| Field | Type | Required / default | Behavior |
|---|---|---|---|
| `source` | nonempty string | required | Existing registry `pack`, `pack@version`, `pack@latest`, or direct source syntax |
| `ref_spec` | nonempty string or null | optional, null | Explicit Git ref; for registry requests requires unversioned `source` and index opt-in |
| `registry_id` | positive i64 or null | optional, null | Pin to managed index; registry requests only |
| `no_registry` | boolean | optional, false | Require an explicit URL/local path, preserving existing direct-source semantics |
| `force` | boolean | optional, false | Replace installed pack; never bypass index/ref/checksum policy |
| `skip_tests` | boolean | optional, false | Existing pack test behavior only |
| `skip_deps` | boolean | optional, false | Existing dependency validation behavior only; no security-policy bypass |

Reject registry `source` containing `@version` together with non-null `ref_spec`.
Reject `registry_id` together with `no_registry: true` or a direct source. Git refs
must also pass Git's applicable ref/object-ID validation, not just the allowlist;
never interpolate them into shell command strings. Exact releases are not semver
ranges, and unsupported selector syntax must produce validation errors.

Examples:

```json
{"source":"core@1.0.2","registry_id":42}
```

```json
{"source":"core","registry_id":42,"ref_spec":"0123456789abcdef0123456789abcdef01234567"}
```

The second request requires Stage B; Stage A returns `feature_not_available` for
registry requests with a non-null `ref_spec`. Direct-source behavior is unchanged.
The second request is not an indexed `version` request. Do not compare the
manifest version to the commit hash, fabricate an indexed release checksum, or
label it checksum-verified. Enforce pack ref identity; record integrity as
unindexed and resolved commit as provenance. Approval of this explicit integrity
exception is required before implementation (section 13).

Lazy refresh handles a missing indexed release or missing pack/repository metadata;
it is not repeated for Git authentication errors or a nonexistent Git ref after
repository resolution. Branch tip changes are resolved by Git for an authorized
explicit-ref install, not by pretending index refresh pins that branch.

## 9. Storage and concurrency design

- Add mode, freshness interval, lazy cooldown, pattern, unindexed flag (constrained
  to false in Stage A), and configuration/source/credential revisions, with
  enum/check constraints for mode/interval consistency.
- Add repository-managed durable snapshot/status storage for managed and static
  identities; persist validated JSON, content digest, revision, fetch timestamps,
  and bounded sanitized errors. Do not store authentication headers in snapshots.
- Keep metadata timestamps distinct from configuration timestamps.
- Atomically publish a new snapshot only if captured configuration/source/credential
  revisions and lease fencing token are still current. Discard obsolete results;
  a failed publication cannot overwrite a newer snapshot/status. Credential rotation
  preserves previously published contents while fencing old in-flight credentials.
- Coordinate refresh across replicas using a DB-backed lease or equivalent bounded
  mechanism. Do not hold a database transaction open while awaiting HTTP.
- Recover abandoned refresh leases; `refresh_in_progress` cannot remain true forever.
- Conditional HTTP revalidation may use ETag/Last-Modified scoped to source identity.
  HTTP 304 advances success time without changing contents/revision; invalid 304
  without a valid snapshot is an error. Byte-identical valid contents likewise need
  not allocate a new revision.
- Implement the shared cooldown, bounded revision-scoped negative lookup cache,
  exponential backoff, and administrator rate/queue limits in section 3.1.1.
  Cooldown/negative-cache decisions must remain consistent across API replicas.
- Store refresh operations/items/idempotency keys in repositories, with atomic
  enqueue and terminal-state transitions. Recover leases without duplicate
  publication; honor bounded retention. No database transactions span HTTP.
- Capture a resolution token containing index identity, configuration revision,
  snapshot revision, selected version/ref, and source checksum/commit. Recheck policy
  before dispatching install work if configuration changed; already-started installs
  are not retroactively cancelled by policy edits.
- Retain revisions referenced by installation provenance, or copy sufficient immutable
  resolution metadata into provenance before snapshot retention removes old contents.

## 10. Errors, security, and observability

Use the existing API error envelope; extend its machine-readable error detail if
needed rather than inventing a parallel wrapper. Proposed domain codes/statuses:

| Code | HTTP | Meaning |
|---|---|---|
| `invalid_index_configuration` | 400 | Bad mode, interval, pattern, URL, headers, or cross-field combination |
| `index_not_found` | 404 | Requested managed ID or static key absent |
| `refresh_operation_not_found` | 404 | Refresh operation absent or expired |
| `idempotency_conflict` | 409 | Reused key with a different request fingerprint |
| `static_index_configuration_conflict` | 409 | Replica/static-manifest definitions disagree |
| `feature_not_available` | 400 | Stage B unindexed refs requested on Stage A |
| `index_disabled` | 409 | Pinned index unavailable for installation |
| `index_not_initialized` | 409 | Manual snapshot absent or invalidated |
| `index_refresh_failed` | 502 | Upstream error or invalid remote document |
| `index_refresh_timeout` | 504 | Fetch timed out |
| `index_refresh_throttled` | 429 | Cooldown, backoff, or refresh rate bound; include `Retry-After` |
| `refresh_queue_full` | 503 | Durable queue capacity reached; include `Retry-After` |
| `index_configuration_changed` | 409 | Repeated config race prevents stable resolution |
| `pack_not_found` | 404 | Pack absent after policy-permitted resolution |
| `pack_version_not_found` | 404 | Explicit release absent after permitted retry |
| `pack_source_unavailable` | 502 | Selected artifact/Git object unavailable; no alternate release substituted |
| `git_ref_not_allowed` | 403 | Pattern rejects selector |
| `unindexed_git_ref_not_allowed` | 403 | Index did not opt into explicit unindexed refs |
| `remote_operations_disabled` | 403 | Global gate denies fetch/install |

These HTTP mappings apply to direct requests and submission failures. After an
operation is accepted, store applicable domain codes in `RefreshItem.error`; status
GET still returns 200 even when the operation failed. Keep existing 401/403
authorization behavior and 413 request-size errors. Import
validation errors include a field path; never return supplied secret values.
Do not conflate upstream HTTP 401 with the caller's Attune authentication status.

Audit configuration changes/imports and refresh outcomes with index identity,
trigger (`manual`, `automatic`, `lazy`), duration, and snapshot revision. Do not
log headers, downloaded content, decrypted secrets, or unsanitized upstream errors.
Expose counts/latency/failures and coalesced refresh metrics with bounded cardinality.

## 11. Implementation sequence

### Stage A: durable snapshots and operational refresh

1. Confirm remaining defaults, standard tracking channel, and deployment protocol.
2. Publish API/config/import schemas and fixtures; pin existing ordering,
   direct-install policy, and checksum behavior in regression tests.
3. Build upgrade preflight and explicit configuration conversion before activating
   new resolver semantics. Add migrations and repository-owned snapshot/job storage.
4. Separate HTTP fetching from snapshot access; implement source/credential fencing,
   static identity ownership, and durable asynchronous refresh operations.
5. Implement automatic/lazy modes, shared cooldown/backoff, bounded negative lookup
   caching, indexed-release selector policy, and immutable provenance using v1 data.
6. Extend API/import/CLI/UI together, with operation polling, idempotent retry,
   explicit invalidation, outage guidance, and migration diagnostics.
7. Generate OpenAPI/web client and YAML-compatible JSON Schema. Validate migration,
   multi-replica behavior, and operator runbooks before rollout.

Stage A can discover a newly published v1 latest version via lazy refresh; it does
not promise installation of historical releases omitted from that snapshot.

### Stage B: additional resolution capabilities

8. Specify and implement bounded historical catalogs/v2 producer support, including
   dependency metadata, source checksums, and a growth/partitioning strategy.
9. Separately review and implement unindexed Git-ref opt-in, immutable commit
   resolution, integrity provenance, and policy-bypass tests. Do not make Stage A
   depend on approval or completion of this security-sensitive feature.
10. Evaluate artifact mirroring and bounded stale-on-error separately; neither is
    implicitly shipped by this plan's initial API/config schemas.

Suggested CLI surface (proposed): `attune pack index refresh <id>`,
`attune pack index refresh --static-key <key>`,
`attune pack index refresh --all [--include-disabled]`,
`attune pack index refresh-status <operation-id>`,
`attune pack index invalidate-snapshot <id>`,
`attune pack index invalidate-snapshot --static-key <key>`, and
`attune pack index import <file>`. Refresh commands print the operation ID promptly;
optional `--wait` polls the same operation and does not submit replacement jobs.
Provide upgrade preflight and static reconciliation commands described below.

Index add/update commands expose the same fields, without credentials in process
arguments where avoidable. UI shows ownership/effective ordering, mode/interval,
cooldown/backoff, pattern, snapshot age/digest, refresh job progress, and outage
behavior. Stage B exposes the unindexed integrity warning only when supported.
Never imply that browsing refreshes an expired snapshot. Distinguish metadata
availability, refresh health, and artifact availability.

## 12. Migration and acceptance

### Migration: deterministic preflight and cutover

Provide a read-only `attune pack index upgrade-preflight --config <file>` command
before cutover. It inventories managed/static definitions and seed URLs, reports
conflicts and initialization needs, validates policies against available metadata,
and emits a redacted conversion proposal. It never fetches, mutates rows/files,
or exposes credentials. An operator explicitly applies the reviewed conversion
through a dedicated upgrade workflow; do not infer legacy/new files by timestamps
or presence of only some new keys.

| Existing condition | Required conversion / resulting behavior |
|---|---|
| Recognized standard seed using a known commit-pinned URL | Replace with approved standard tracking channel, manual mode, numeric-triplet pattern, unindexed false; require explicit first refresh |
| Operator-customized standard URL | Preserve URL; explicitly confirm whether it tracks releases or intentionally pins metadata; manual defaults remain |
| Nonstandard managed index, legacy cache enabled, TTL in `1..604800` | Preflight proposes automatic + that TTL, never silently applies it; operator approves mode and selector policy per index |
| Legacy static entry | Assign explicit stable `key`, mode, applicable interval/cooldown, and reviewed pattern; reconcile the accepted static manifest before starting new replicas |
| Legacy TTL absent with caching enabled | Preflight uses the old documented 3600-second default for its automatic proposal |
| Legacy TTL outside new bounds or cache disabled | Block automatic conversion; require explicit manual/lazy/automatic selection and valid bounds; no clamping or fictitious every-install mode |
| Explicit legacy `cache_ttl` or `cache_enabled` keys remaining | New startup rejects them with conversion guidance; remove after converting policies |
| Existing nonstandard index without approved conversion | Keep its migration state pending; block activation of the new index resolver with an actionable diagnostic, not accidental manual defaults |
| Brand-new instance/index | Apply section 5/7 create defaults: manual, numeric-triplet pattern, unindexed false |
| Disabled existing index | Preserve disabled state, still validate/convert its policy; no implicit fetch |

Database migrations perform schema/local-data work only, never HTTP. Persist an
instance-level conversion-complete marker after all managed and static definitions
have passed preflight and explicit acceptance, so restart behavior is deterministic.
Old binaries must be drained before activating new resolver semantics; a mixed
old/new deployment must not let old processes bypass manual-only policy.

Cutover runbook:
1. Back up configuration and database; inventory network approvals and rollback path.
2. Run preflight against the old effective configuration, including defaults; review
   nonstandard policies, seed-channel changes, and any credential scope changes.
3. Pause installations/refresh work, drain old replicas, apply local schema changes
   and approved conversions, and register the agreed static manifest.
4. Start matching new replicas; reject unresolved conversion/conflict diagnostics.
5. Explicitly initialize manual/public snapshots via refresh jobs, inspect per-index
   results, then resume installations. Automatic/lazy may fetch on first install,
   but administrators can prewarm them with the same job interface.
6. Confirm saved snapshots/revisions survive a restart and run representative installs.

Transient old caches cannot be recovered reliably and are not considered initialized
snapshots. A bundled trusted public snapshot could reduce bootstrap downtime, but
requires a separately reviewed provenance path. No hidden startup fetch is allowed.
Validate indexed SHA sources against their release selectors, not against the public
selector regex. Preflight warns about nonstandard version names before deployment;
it must not silently broaden a restrictive allowlist to make validation pass.
Downgrade to binaries without manual-policy enforcement is an operator-visible
behavior change, not a transparent rollback; keep remote installs paused until
configuration/schema compatibility and desired policy are explicitly restored.

### Acceptance matrix

- Manual install/browse does no network fetch, even after TTL expiry or restart.
- Manual without snapshot produces initialization guidance; standard refresh uses
  the tracking channel and can discover releases beyond the previously seeded commit.
- Ordinary credential rotation preserves saved manual installs; URL changes and
  explicit tenant/scope invalidation fence old snapshots and in-flight requests.
- Automatic hits before expiry do not fetch; at expiry fetch before resolution.
- Lazy missing version fetches once and retries; successful cached resolution does
  not fetch; absent pack and initial empty snapshot have bounded attempts.
- Manual/automatic/lazy `latest` semantics match section 3.
- Snapshots/status are shared across replicas and survive process restarts.
- Single/batch refresh returns 202 promptly, survives disconnect/restart, exposes
  per-target progress and partial failure, and safely deduplicates idempotent retries.
- Lease recovery/configuration races cannot double-publish or refresh a replaced
  target; queue bounds and operation retention are exercised.
- Stable static keys support targeted inspection/refresh/invalidation; mismatched
  replica definitions produce conflicts rather than credential/snapshot oscillation.
- Sequential nonexistent-version probes are bounded by shared cooldown, backoff,
  and revision-scoped negative caching; confirmed miss and deferred refresh differ.
- Upstream outage after automatic expiry fails closed with explicit guidance;
  switching to manual is an audited decision, not a silent fallback.
- Migration tests cover every conversion-matrix row, explicit legacy keys, mixed
  replica prevention, public bootstrap, and customized URLs.
- Failed/invalid fetches cannot replace good data; HTTP 304 works only with a valid
  snapshot. Config races cannot publish obsolete contents.
- Disabled, priority, duplicate URL, pinned ID, and dependency behavior are tested.
- Default public releases accept `1.0.2` and `12.10.3`, reject `v1.0.2`, branches,
  prereleases, and user-supplied hashes, yet install indexed immutable SHA sources.
- Stage A rejects unindexed opt-in/ref requests as unsupported. Stage B private
  explicit commit refs require both allowlist and unindexed opt-in; integrity
  provenance is truthful. Fallback/direct paths cannot claim policy verification.
- Catalog growth is bounded; Stage B producer/schema tests cover history limits and
  generation consistency. Missing upstream artifacts fail distinctly even when
  metadata exists; never claim a saved snapshot alone guarantees reproducibility.
- Regex compilation, full matching, invalid patterns, interval bounds, null/omission
  semantics, unknown/duplicate YAML keys, oversized bodies, and secret redaction
  have positive and negative tests.
- Import is atomic, non-destructive to omitted entries, and never fetches implicitly.
- API and YAML fixtures normalize to the same effective configuration.
- Auth constraints protect create/update/import/delete/refresh and inspection paths.
- Generated client/schema changes and UI configuration workflows are tested.

During implementation run targeted Rust/API/CLI/UI tests, `cargo sqlx prepare`
after schema changes, and `cargo check --all-targets --workspace` with no new
warnings. Use repository test-isolation conventions. Documentation-only delivery
of this plan does not require building or migrating the workspace.

## 13. Review items before implementation

The operational recommendations are incorporated above. The following remaining
specifics need implementation-time review; no runtime changes are authorized by
saving this document:

1. Confirm the maintained standard tracking channel with its producer; keep manual
   refresh as the default regardless of channel selection.
2. Confirm proposed numeric limits: automatic default 3600 seconds, lazy cooldown
   30 seconds, seven-day interval cap, operation retention, queue bounds, and backoff.
3. Specify the internal static-manifest acceptance/deployment protocol and preflight
   conversion interface before rollout; retain the no-last-writer-wins invariant.
4. Publish the complete bounded historical catalog v2 schema with producers before
   Stage B, including history partitioning/retention and dependency placement.
5. Review Stage B's unindexed integrity exception independently of Stage A. If
   actual tag/branch/commit-kind restrictions are needed, specify a separate policy
   and Git-resolution checks; regex inference is not sufficient.
6. Finalize the browse snapshot-status envelope and consumer migration together.
7. Artifact mirroring and bounded stale-on-error remain separate extensions; initial
   deployment docs must clearly state upstream availability requirements.

The agreed interval-based automatic mode and independent per-index policies remain
unchanged. Stage A can be delivered without unindexed Git refs or artifact mirroring.
