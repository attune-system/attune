# Public declared-input consent

Input consent is separate from runtime permission selection. It authorizes fixed
reads for one retained declaration, not SDK access, execution creation or a human
delegation fallback. All routes below require an Access token and fresh
`packs:read` on the subject pack. Issuance and revocation retain their existing
issuer-coverage, fixed-expiry and explicit-revocation checks.

## Discover a target before execution

`GET /api/v1/packs/{pack_ref}/input-approval-targets`

Query parameters:

- `policy_revision`, optional positive ID. Omit for the pack's current policy.
  Explicit IDs select exact retained metadata, not today's equivalent refs.
- `page`, positive, default 1.
- `page_size`, 1 to 100, default 20.

The standard paginated response contains exact public target summaries:

```json
{
  "policy_revision": 17,
  "invocation_binding": {"kind": "action", "action_ref": "example.flow"},
  "action_id": 23,
  "action_ref": "example.flow",
  "pack_id": 11,
  "pack_ref": "example",
  "release_id": 15,
  "metadata_revision": null,
  "declaration_revision": "exact-retained-declaration-digest",
  "source_path": "/workflow/vars/token",
  "expression": "{{ keystore['foreign.token'] }}"
}
```

Source components and the declaration's actual action must also be currently
visible. Actions/workflows use their existing public visibility or actual
`actions:read`; rule and queue sources require their corresponding read grant.
Structural IDs, owners and retained refs must agree. A public leaf does not expose
a private calling workflow, rule or queue. Current-name replacement and mere pack
association do not repair a missing original component.

Authorized invocation digests and declaration subject ownership filter rows and
counts before ordering and pagination. Expressions are authored metadata, not
evaluated values. Discovery neither reads Keys/configuration values nor creates
an execution or consent. The existing current action/workflow APIs do not expose
the complete retained policy/declaration identity required for consent issuance.

## Issue exact declaration-scoped consent

`POST /api/v1/packs/{pack_ref}/external-input-approvals`

Use either `renderer_execution` for an existing admitted renderer, or
`policy_revision` plus canonical `invocation_binding` before execution. The two
strict flat request variants are exclusive. Both also require
`declaration_revision`, `source_path`, `expression`, authored flat `grants` and
optional RFC3339 `expires_at`. Copy those exact fields from discovery, not the
summary's extra action/pack/release identities.

Canonical invocation variants are `action`, `workflow_task`, `rule` and `queue`.
Their component refs and task path have the same typed schema as admitted
invocations. Grants use `PermissionGrantDefinition`, not opaque JSON objects.
Authored constraints omit server-owned `ids` and `managing_pack_id`; ordinary
fixed-scope validation still rejects them if manually supplied. Input consent
permits only `keys`/`packs` `read`/`decrypt` with fixed external scope. It has no
caller-selected `purpose`, private proof, SDK `permission_set_refs` or source value.

## Inspect and revoke without reconstructing proof

The shared ledger remains:

- `GET /api/v1/packs/{pack_ref}/external-approvals`
- `GET /api/v1/packs/{pack_ref}/external-approvals/{id}`
- `POST /api/v1/packs/{pack_ref}/external-approvals/{id}/revoke`

Existing lifecycle, issuer, scope, digest and active-status fields keep their
semantics. An additive typed `purpose` field distinguishes:

```json
{"kind": "runtime"}
```

```json
{"kind": "declared_input", "targets": []}
```

The input variant contains up to 256 currently visible exact retained target
summaries. The array is empty when old target rows have been collected or current
component visibility denies disclosure. The input-only marker remains input-only;
absence never becomes runtime authority. Hidden target counts are not exposed.
Ledger membership and total counts remain unchanged, including expired/revoked
records. Malformed retained evidence fails rather than inventing current targets.

Only the public summary whitelist is returned. Receipts, private origin proofs,
concrete-target capability bindings, ciphertext, Key values and source
configuration are not serialized. Revoked/expired metadata inspection does not
consume authority or check the old issuer's current human activity. Runtime use
continues to require the original dependencies, expiry and revocation checks.

CLI discovery is `attune pack input-approval targets`; MCP discovery is
`pack_input_approval_targets_list`. Both use the public API and existing profile
transport. Generators should consume this canonical v1 schema once central
integration freezes it, not add a second wire format or SQL discovery workaround.
