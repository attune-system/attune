# Artifact metadata, selected content, and browsing

The canonical v1 artifact collection contract uses bounded cursor pages:

```text
GET /api/v1/artifacts?per_page=20
GET /api/v1/executions/42/artifacts?per_page=20
GET /api/v1/artifacts/17/versions?per_page=20
```

```json
{"items": [], "next_cursor": null, "has_more": false}
```

`per_page` defaults to 20 and must be between 1 and 100. Follow `next_cursor` with
the same endpoint and filters to continue. Cursors are opaque selection state,
not authorization: current permissions apply on every request. Malformed or
scope/filter-mismatched cursors return 400. There is no alternate page-number or
unbounded `data` response. Regenerate typed clients from the current OpenAPI.

Artifact ordering is `created DESC, id DESC`; version ordering is
`version DESC, id DESC`. Browse requests do not calculate exact totals or total
pages. `has_more` describes continuation, not an estimate of history size. A
recorded-credential scan can return an empty page with a continuation cursor when
its bounded candidate batch contains no authorized rows; continue until
`next_cursor` is null.

## Human runtime-log metadata and content

A shared action-level stdout/stderr artifact is a container of versions produced
by different executions. Its size is not a receipt-graph size. Human Access-token
browsing separates safe metadata visibility from plaintext disclosure:

- Global and execution artifact summaries expose permitted metadata, not a body
  disclosure decision for every historical execution.
- A runtime-log detail GET returns an explicitly restricted header: artifact
  identity/ref, owner/scope, type/visibility/classification, name, timestamps, and
  retention settings. It excludes arbitrary data/descriptions and storage/source
  records. Aggregate latest size/content type are not an unconditional header grant.
- Execution summaries select the highest version for that exact artifact and
  requested execution. `selected_version` identifies it and supplies visible
  version-specific display fields. It does not select a later run's output.
- Version history exposes only metadata-visible versions. A listed version is
  not a promise that the caller may read its content.
- Content reads authorize the selected version, its execution or sensor
  generation, and every required recorded origin. An unrelated old execution
  cannot authorize or block that selected content. Missing required sources,
  bindings, cycles, and excessive source graphs still fail closed.

For a specific version use `/api/v1/artifacts/{id}/versions/{version}/download`.
Pass `version_id` from its summary to pin the immutable row across requests;
an ID mismatch is not found rather than a fallback to a reused version number.
Execution stdout/stderr live reads use
`/api/v1/executions/{id}/logs/{stream}/stream`. Latest reads select one newest
metadata-visible version and then authorize that body; they do not search older
history to bypass a denied latest body. Explicitly select an older visible version
when that is the intended read.

Protected content denial remains masked as not found. It does not imply that a
permitted metadata header is absent. Database/internal failures remain real
request failures rather than successful empty results or masked disclosure denials.
Stream frames and guarded download chunks retain live revocation/freshness checks.
Ordinary artifact data/progress reads retain their existing protected semantics.

The execution endpoint accepts an optional `type` filter. Progress discovery uses
`?type=progress&per_page=1`, so it does not depend on where progress appears in an
unfiltered page. The execution identity always comes from the URL path.

## Live browsing and CLI output

Cursor pages are not a multi-request database snapshot. Concurrent insertion and
retention can change the browse. Browser live refreshes discard old continuation
state and return to the first page. Pause updates to browse older pages; resuming
starts a new browse. Previous navigation uses the client's cursor stack, not an
inferred page number or total.

`attune artifact version list` retains its full-history output by traversing
server cursors. JSON/YAML output remains an array. It does not silently truncate
at the server's first page. Traversal is not a snapshot across concurrent changes.

See [the selected-read implementation plan](../plans/execution-scoped-artifact-reads.md)
for the reproduced 257-execution defect, policy decisions, and validation gates.

## Typed consumers

Backend, web, CLI, and generated clients must adopt the cursor contract together.
External Python/JavaScript/Java SDK consumers must regenerate or update their
artifact response parsers: read `items`, follow `next_cursor`, and do not expect
`pagination.total_items`, `pagination.total_pages`, or page-number queries.
SDK source changes in other repositories and package publication are separate
release actions; in-tree verification does not certify those external packages.

## Recorded execution credentials

Recorded reads use structural database owners and pinned `standard` owner IDs,
not requester IAM or action-ref prefixes. All linked executions must be readable;
secret-bearing content also requires execution decrypt and every retained origin.
Missing bindings, cycles, excessive origin graphs and unavailable source owners
deny disclosure. Sensor-generation, sensor-delivery and inquiry origins remain
closed until their typed disclosure consumers are connected.

Migration `20261015003001` records owner IDs when artifacts are created or their
owner is explicitly changed. A harmless metadata edit cannot rebind an old
artifact after an action ref is reused. Historical rows without this proof stay
closed to recorded credentials; the migration does not infer their owners from
current names. Recorded credentials do not borrow the human metadata/content
policy above. Retained identity ownership does not depend on that human account
remaining present.

Recorded plaintext delivery must use the existing typed acquisition registry,
transaction-scoped selection, retained origins, and route-specific credential
restrictions. A human metadata grant is not a recorded content permit. Unsupported
consumers remain closed; this change does not enable a previously denied recorded
version-content, history, download, or SSE operation. Immutable-ID assertions are
checked before body acquisition or runtime-read provenance capture, including
recorded history candidates.

Recorded collection reads authorize bounded candidate batches and may return an
empty resumable cursor page when candidates are denied. They do not perform an
exact count or inherit the human selected-version scalability claim. Each
artifact retains a maximum of 256 linked executions, and origin traversal permits
at most 256 nodes, 32 levels and 1 MiB of encoded provenance. Overflow fails closed.

Create, metadata update/delete, progress append, data replacement, JSON version
creation and version deletion retain the credential and target locks through
commit. Owner transfers require authority on the destination too. Recorded JSON
versions retain the credential's own execution attribution and cannot claim a
different execution. Mutation responses omit existing artifact data. File
allocation/upload mutations remain denied to recorded credentials until
converted, and production worker issuance remains gated on complete consumer and
completion readiness.
