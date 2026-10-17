# Artifact list and history reads

The sibling Python and JavaScript SDK trees expose typed paginated responses;
offline contract tests exercise both page queries and response parsing. Java's
generic client retains the response map, so caller-owned deserializers must use
`items` and `pagination`. Each SDK has an artifact-pagination migration notice.
These source/tests do not constitute package publication or deployment.

These v1 endpoints return `PaginatedResponse` with `items` and `pagination`:

```text
GET /api/v1/artifacts?page=1&per_page=20
GET /api/v1/executions/42/artifacts?page=1&per_page=20
GET /api/v1/artifacts/17/versions?page=1&per_page=20
```

The default page is 1 and default page size is 20. `per_page` must be between 1
and 100. Page zero, invalid sizes, and offsets exceeding the supported 32-bit
range return 400 before artifact/version selection reads. The execution and
version endpoints no longer return an unbounded `data` array. Regenerate typed
clients from the current OpenAPI export when adopting this contract.

Pagination includes exact `total_items` and `total_pages`, plus `has_previous`
and `has_next`. An out-of-range page returns no items and still reports its exact
total. Artifact pages sort by `created DESC, id DESC`. Version pages sort by
`version DESC, id DESC`.

## Eligibility and totals

The global list retains SQL visibility filtering. It does not add the runtime-log
disclosure gate used by execution lists and detail reads. A caller can therefore
see a global log summary but receive 404 for its detail.

Execution lists apply visibility and the existing all-linked-execution disclosure
gate before selecting a page. Their exact total counts disclosure-eligible
artifacts, not merely SQL-visible candidates. Denied leading candidates do not
hide later readable artifacts. The API scans candidates in bounded internal
batches and retains only the requested page, but exact totals still require
authorizing the full candidate set and relevant linked history. Small responses
do not imply constant total authorization work.

Version history first applies the artifact detail gate, then uses the existing
version-visibility predicate for both its count and page. Latest-readable-version
selection, downloads, and stream revocation checks are unchanged. Repository and
internal failures remain request failures, not invisible artifacts or masked
detail denials.

The execution endpoint accepts an optional `type` filter. Progress discovery uses
`?type=progress&per_page=1`, so it does not depend on where progress appears in an
unfiltered page. The execution identity always comes from the URL path.

## Live browsing and CLI output

Pages use offsets, not a stable snapshot or historical cursor. Inserts and
retention can shift page boundaries between requests. Browser live refreshes
discard old page results and return to page one. Pause updates to browse older
pages; resuming starts a new browse at page one.

`attune artifact version list` retains its full-history output by traversing
server pages. JSON/YAML output remains an array. It does not silently truncate at
the server's first page. Like browser offset pages, traversal is not a snapshot
across concurrent changes.

See [the performance implementation checkpoint](../plans/artifacts-postgresql-implementation.md#implementation-checkpoint)
for query-growth evidence, pending source-scope batching, and performance gates.

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
current names. Human artifact reads keep their existing contract. Retained
identity ownership does not depend on that human account remaining present.

Protected plaintext delivery remains closed even when those checks pass. The
current runtime SDK-read registry records Keys only; it cannot retain every
source execution/event/queue gate when an action forwards artifact content.
Recorded version-content/history, download and SSE routes remain unmarked and denied
until that provenance and transaction-scoped version selection are connected.
The converted reads are artifact metadata/data and bounded artifact summary
collections, with sensitive linked content denied.

Recorded collection reads scan lightweight candidates in 256-row batches and
authorize before pagination and exact counting. They retain only the requested
page. This path has linear authorization query cost in the candidate count; it
does not inherit the batched human-reader query-growth claim. Each artifact has
a maximum of 256 linked executions, and origin traversal permits at most 256
nodes, 32 levels and 1 MiB of encoded provenance. Overflow fails closed.

Create, metadata update/delete, progress append, data replacement, JSON version
creation and version deletion retain the credential and target locks through
commit. Owner transfers require authority on the destination too. Recorded JSON
versions retain the credential's own execution attribution and cannot claim a
different execution. Mutation responses omit existing artifact data. File
allocation/upload mutations remain denied to recorded credentials until
converted, and production worker issuance remains gated on complete consumer and
completion readiness.
