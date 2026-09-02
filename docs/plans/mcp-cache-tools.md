# MCP Cache Tools Plan

## Goal

Expose bounded, owner-scoped data-cache operations through `attune-mcp` without
weakening the cache API's authorization or placing unbounded datasets into an
MCP client's context.

This plan is subordinate to the canonical policy in
[`KEY_CACHE.md`](../KEY_CACHE.md#mcp-cache-tool-policy). If this plan and that
policy differ, `KEY_CACHE.md` governs.

## Scope

Add MCP tools for the cache operations already available through `attune cache`:

- Namespace lifecycle:
  - `cache_namespaces_list`
  - `cache_namespace_get`
  - `cache_namespace_create`
  - `cache_namespace_update`
  - `cache_namespace_delete`
- Published data reads:
  - `cache_entry_get`
  - `cache_entries_get_many`
  - `cache_entries_scan`
- Generation inspection:
  - `cache_generations_list`
  - `cache_generation_get`
- Copy-on-write refresh lifecycle:
  - `cache_refresh_begin`
  - `cache_refresh_upload_chunk`
  - `cache_refresh_seal`
  - `cache_refresh_promote`
  - `cache_refresh_abort`

## Tool Contracts

`cache_namespaces_list` lists every namespace that the authenticated identity
can read when `owner_type` is absent. The remaining tools require an explicit
`owner_type`. Pack, action, and sensor owners require their matching reference.
Identity ownership always resolves to the authenticated identity and rejects
`owner_ref`. This matches the CLI and cache API ownership model.

Cache scans will return one bounded page at a time and preserve the API cursor
and generation pinning. They will default to metadata-only output; callers must
explicitly set `include_values` to read values. Point and multi-entry lookups
will return values because that is their intended purpose.

Generation IDs will use integer schemas compatible with Attune's `i64` IDs.
Schemas will bound page sizes, multi-lookup IDs, chunk indices, and refresh
payload sizes to the limits enforced by the API.

Refresh uploads will accept structured entry arrays, not paths to NDJSON files.
An MCP client cannot safely assume access to the MCP server's filesystem, and
structured bounded chunks map directly to the cache upload API.

The refresh tools form a bounded, structured, multi-call lifecycle. Each call
begins one generation, uploads one bounded chunk, seals, optimistically
promotes, or aborts; no call accepts or returns a complete dataset.

## Intentionally Excluded Operations

The initial MCP implementation will not expose equivalents of:

- `attune cache entry scan --all --output ndjson`
- `attune cache refresh apply --input <ndjson>`
- automatic cursor-following or any full-dataset response
- filesystem-based refresh input or apply
- force promotion

The CLI bulk operations are designed for streaming arbitrary datasets through
stdout or a local file. Exposing them to MCP would risk placing unbounded cache
data into agent context or relying on an undefined shared-filesystem model.
Automatic cursor-following would have the same unbounded-response risk, while
force promotion would bypass the normal optimistic publication precondition.
One bounded scan page per call and the bounded structured refresh lifecycle
provide safe multi-call access without adding those operations.

## Implementation

1. Add the cache tool definitions, descriptions, and JSON input schemas in
   `crates/cli/src/bin/attune-mcp.rs`.
2. Add corresponding `McpServer::call_tool` dispatch branches.
3. Use the existing `ApiClient` cache methods (`cache_get`, `cache_post`,
   `cache_put`, `cache_patch`, and `cache_delete`) so cache response envelopes
   and typed API errors are handled consistently with the CLI.
4. Build URLs and bodies from the existing cache API contract rather than
   duplicating persistence or authorization logic.
5. Clearly label destructive operations and preconditions in descriptions:
   namespace deletion, refresh abort, and the mutually exclusive
   `expected_active` / `expect_empty` promotion requirements.

## Security and Authorization

MCP will use the caller's normal API token and will not introduce elevated
authorization. The API remains responsible for ordinary cache RBAC, including
signed sensor-cache authority and owner/namespace constraints. Read tools
require the applicable cache-read authority. Refresh creation, chunk upload,
sealing, promotion, and abort require explicit cache-write grants. Tool
descriptions must state that cache values can contain sensitive business data.

## Validation

Add MCP tests that verify:

- Every planned cache tool appears in `tools/list`.
- Schemas require owner selection, reject unknown fields, and enforce bounds.
- Representative read, write, URL-encoding, and cache-error paths dispatch to
  the expected cache API route using a local mock API server.
- Scans have no unbounded mode and do not include values by default.
- No cache tool bypasses normal API authorization.

Update MCP/CLI documentation with the tool list, bounded-operation rationale,
and examples for reading a value and executing a multi-call refresh.
