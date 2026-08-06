# Data Caches Web Experience

This document describes the **Data Caches** area of the Attune web UI: an
owner-scoped, generation-based external-data cache viewer/operator surface
that is deliberately separate from **Keys & Secrets**. See
[`docs/KEY_CACHE.md`](../KEY_CACHE.md) for the full design rationale and the
backend data model this UI is built against.

Data Caches are Attune-local, reconstructable automation snapshots. The web UI
does not present them as authoritative business storage, a general query
database, or a secret store. The source system remains authoritative, and
operators should be able to rebuild every namespace from it.

## Status

`crates/api/src/routes/cache.rs` and `crates/api/src/dto/cache.rs` are included
in `ApiDoc`. Their cache models and `CachesService` are generated under
`web/src/api/`; the web client has no handwritten cache wire DTO or request
facade.

## Why a separate area from Keys & Secrets

Keys and Secrets remain focused on small, mutable configuration values and
credentials (`attune key`, `SecretsService`, `useKeys`, the `/keys` route).
Data Caches are a distinct **read-mostly, high-cardinality, generation-based**
dataset: no encryption checkbox, no key reference, no inline value editor, and
no shared list/detail pages with Keys. This mirrors the RBAC split — a
dedicated `Resource::Caches` rather than an extension of `Resource::Keys` (see
`crates/common/src/rbac.rs`).

## Routes

| Path | Page | Notes |
| --- | --- | --- |
| `/caches` | `CachesPage` | Owner-scoped namespace index with bounded client-side text/status filtering. |
| `/caches/:ownerType/:ownerRef/:namespace` | `CacheNamespaceDetailPage` | Tabbed detail view (Overview / Records / Generations / Refresh). |

`:ownerType` is one of `system | identity | pack | action | sensor`.
`:ownerRef` is the denormalized owner reference (e.g. a pack ref) for
`pack`/`action`/`sensor` scopes. `system` and `identity` scopes have no
meaningful denormalized ref, so the route/service layer use reserved
placeholders instead of a real ref string:

- `system` → `_`
- `identity` → `self` (the initial scope only supports the caller's own
  identity; see `docs/KEY_CACHE.md`)

Note that this owner-in-the-path shape is a **web-route-only** convention. The
actual API never puts owner scope in the URL path — every cache route takes
`owner_type` (required) + `owner_ref` (optional) as query params (GET/DELETE)
or request-body fields (POST/PUT), with only `{namespace}` (and
`{generation_id}`/`{chunk_index}` where relevant) as path segments. There is
also no cross-owner "list all namespaces" endpoint: `GET /cache/namespaces`
requires an owner scope and returns a bounded keyset page with `next_cursor`.
It accepts `limit`, `cursor`, and optional namespace/freshness filters. This is
why `CachesPage` requires picking an owner scope (defaulting to `system`, which
needs no further input) before it lists anything.

`web/src/components/caches/cacheUtils.ts` (`buildCacheNamespacePath`,
`parseOwnerRouteParams`, `ownerRefForPath`) is the single source of truth for
the web-route encoding/decoding so the index, detail page, and any future deep
links stay consistent.

## Navigation and permissions

- **Data Caches** is a sibling nav item to **Keys & Secrets**, not a tab
  inside it (`web/src/components/layout/MainLayout.tsx`).
- The RBAC resource key is `"caches"` (`web/src/lib/permissions.ts`). It is
  **not** included in `AUTHENTICATED_DEFAULT_READ_RESOURCES` — unlike
  `packs`/`actions`/etc., an authenticated user does not get implicit read
  access to caches. Navigation/controls are hidden or disabled without an
  explicit grant, but this is a UX convenience only: the API is the
  authorization authority for every request.
- The permission-set editor (`web/src/pages/access-control/PermissionSetDetailPage.tsx`,
  logic in `web/src/pages/access-control/grantDraft.ts`) exposes `caches` with
  `read | create | update | delete` actions, an owner-type constraint
  (`owner_types`), an owner-**reference** constraint (`owner_refs` — new;
  previously unused by any other resource), and a namespace constraint via the
  generic `refs` field (labeled "Namespace refs" for this resource). Owner-only
  grants (no namespace ref) cover every namespace in that owner scope.

## Tabs

1. **Overview** — namespace policy (freshness target, quotas), active
   generation summary, quota usage, and a danger zone (delete namespace with
   count/byte impact and a typed-namespace confirmation). Capacity indicators
   describe cache consumption; they do not imply that Attune owns the source
   data. Persistent capacity, WAL/backup, or cleanup pressure is an operational
   signal to isolate or extract the workload as described in `KEY_CACHE.md`.
2. **Records** — exact external-ID lookup, bounded multi-ID lookup (capped at
   1,000 IDs client-side, mirroring `MAX_MULTI_LOOKUP_IDS` in
   `crates/common/src/repositories/cache.rs`), and generation-pinned
   cursor-page browsing. There is no offset paging and no "load all" action.
   Browsing is gated behind an explicit "Start browsing" click (deliberate
   access), shows the pinned generation and cursor expiry, and offers
   **Restart on current generation** plus a separately-labeled "start a new
   browse" action when a snapshot expires.
3. **Generations** — read-only lifecycle metadata (state, counts, bytes,
   source revision, creator/client-refresh ID, timestamps) with an
   expandable detail row showing the failure reason for a failed generation.
   The API has no per-generation ingest-chunk listing endpoint, so chunk-level
   detail is not shown here — only the generation's own authoritative counts.
   No entry editing.
4. **Refresh** — bounded, resumable browser NDJSON-file upload; seal; an
   old-vs-new generation review; optimistic promotion with visible conflict
   handling; and an explicit abandon confirmation (the server records a fixed
   audit reason for abandon — it does not accept a caller-supplied one). A
   "force promote" affordance is visually distinct but intentionally
   disabled — see
   [Force promotion](#force-promotion-is-intentionally-unavailable).

Cache values are only ever rendered inside an explicit Records lookup result
(point lookup, multi-ID lookup, or a browsed page). The namespace index,
generation list, confirmation dialogs, and error messages never include entry
values or raw external-ID lists.

## Hooks and query keys

`web/src/hooks/useCaches.ts` exposes `useCacheNamespaces`, `useCacheNamespace`,
`useCacheGenerations`, `useCacheGeneration`, `useCacheEntryLookup` /
`useCacheEntriesGetMany` (mutations — deliberate, not ambient),
`useCacheEntryScan`, and the refresh lifecycle mutations
(`useBeginCacheRefresh`, `useUploadCacheChunk`, `useSealCacheGeneration`,
`usePromoteCacheGeneration`, `useAbandonCacheGeneration`).

The `cacheKeys.entryScan(owner, namespace, { generationId, cursor, limit })`
query key threads namespace, generation, cursor, and page shape (`limit`) all
the way through, so a promotion or page-size change can never serve a stale
or mismatched page from the React Query cache.

## Workflow Builder and Execution Views

The workflow builder represents native cache iteration as an `iterate_cache`
task block; it does not translate it into `with_items` or generate an action
that calls the cache API. The block supports owner type/ref, namespace,
`generation` (default `active`), `require_fresh` (default `false`), and internal
scan `page_size` (default `100`, range `1..1000`). Task `batch_size` is supported
independently, defaults to `1`, and controls whether `item` is one entry object
or an array of entries. `index` is the batch ordinal.

The workflow execution details panel renders the safe implemented subset: task
name, state, generation ID, scanned/dispatched counts, batch and page sizes,
concurrency, timestamps, and a bounded error summary. It does not show leases,
freshness, page retry summaries, failed page indexes, or cache records.

Authorized clients can use
`GET /api/v1/executions/{id}/workflow-cache-iterations`. Its response also
includes `namespace_id` and is otherwise limited to the same safe status data.
Cursor state, external IDs, and values are omitted.

## Regenerating the client

Run `npm run generate:api` from `web/`. The script exports `ApiDoc` directly
with the repository's `export-openapi` binary, so a running API server is not
required. It refreshes `web/openapi.json`, generates into a temporary directory,
and replaces only the generator-owned `web/src/api/core`, `models`, `services`,
and `index.ts` paths. Top-level legacy extension files are therefore not
silently erased.

Cache code follows the normal generated-client boundaries:

- Wire DTOs and enums are generated in `web/src/api/models/`.
- Requests use the generated `web/src/api/services/CachesService.ts`.
- React Query adaptation and owner-argument mapping live in
  `web/src/hooks/useCaches.ts`.
- UI-only owner and JSON types plus error-code classification live outside the
  generated tree in `web/src/types/cache.ts` and
  `web/src/components/caches/cacheUtils.ts`.

The generated contract includes these important details:

- Every route requires `owner_type` (never optional) + optional `owner_ref`,
  as query params for GET/DELETE and request-body fields for POST/PUT. Only
  `{namespace}` / `{generation_id}` / `{chunk_index}` are path segments.
- Chunk upload is `PUT .../generations/{generation_id}/chunks/{chunk_index}`
  with a JSON body `{owner_type, owner_ref, entries: [...]}` — not raw NDJSON
  text. The server derives its idempotency checksum from the raw request
  bytes itself; the client does not compute or send one.
- Cache-specific error conditions arrive in the response body's `code` field
  (e.g. `snapshot_expired`, `cache_not_populated`,
  `cache_precondition_failed`), not `error` (which is the human-readable
  message), almost always alongside HTTP 409 — see
  `web/src/components/caches/cacheUtils.ts`'s `isSnapshotExpiredError` /
  `isCacheNotPopulatedError` / `isPromotionConflictError`.

Do not edit generated cache files directly. Change the Rust DTO/path metadata,
regenerate, then update hooks or UI conveniences only when the generated
method signatures change.

## Force promotion is intentionally unavailable

`docs/KEY_CACHE.md` treats administrative force promotion as a separate,
strongly authorized, prominently audited operation, distinct from ordinary
optimistic promotion, and it is not part of the required initial cache RBAC
scope. The Refresh tab therefore renders a visibly distinct, disabled "Force
promote" control with an explanatory tooltip rather than fabricating a
client-side bypass for a server contract that does not exist yet. When a
promotion conflict is detected (HTTP 409), the UI surfaces the conflict and
refetches the namespace so the operator can see the new active generation
before deciding what to do next.

## Testing

Focused tests live alongside the feature:

- `web/src/components/caches/cacheUtils.test.ts` — namespace naming, owner
  path encoding/decoding, status/badge computation, error-code classification
  (`code` field, not `error`), and bounded NDJSON line-splitting/chunking/
  streaming (including a real `Blob`-based streaming test).
- `web/src/hooks/useCaches.test.tsx` — query key shape (namespace / generation
  / cursor / page shape) and deliberate-access gating for entry scans.
- `web/src/pages/access-control/grantDraft.test.ts` — cache grant ⇄ draft
  round-tripping, including `owner_refs` + namespace `refs` scoping.
- `web/src/lib/permissions.test.ts` — `/caches` route requirement and the
  "not a default authenticated read resource" invariant.
- `web/src/components/access-control/GrantsView.test.tsx` and
  `web/src/components/caches/CacheConfirmDialog.test.tsx` — component-level
  rendering/interaction checks.

Run them with `npm run test` (or `npm run test:watch`) from `web/`.
