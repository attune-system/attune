# External runtime approvals

Implementation status: approval storage and control endpoints are available. Automatic deployment capture and production execution consumers are not yet connected. Creating an approval alone does not change an existing execution's runtime permissions. See the [implementation tracker](../plans/self-contained-pack-implementation-progress.md).

An approval gives a named pack fixed external capabilities. It does not assign permission sets to a human. The issuer must currently cover every approved grant or have unrestricted permission-administration authority. Inspecting the subject pack requires Packs/Read. Controls accept access tokens, including integration access tokens, but not execution or sensor tokens.

The approved subject and grants are immutable. Expiry is optional. Freezing, deleting, or changing the issuer's IAM does not revoke the approval. Use explicit revocation to stop its recorded runtime effects.

## API

All paths are beneath `/api/v1/packs/{pack_ref}/external-approvals`:

| Method and suffix | Operation |
| --- | --- |
| GET | List approvals, including expired/revoked records. Uses `page` and `page_size`, with at most 100 records per page. |
| GET `/{id}` | Inspect fixed scope, historical issuer, expiry, digest and revocation. |
| POST | Issue an approval after fresh authority validation. |
| POST `/{id}/replace` | Issue a new fixed approval and explicitly revoke the predecessor in one transaction. |
| POST `/{id}/revoke` | Revoke idempotently without deleting the historical record. |

Issue and replace accept the same JSON body:

```json
{
  "grants": [
    {
      "resource": "events",
      "actions": ["create"],
      "constraints": {"refs": ["shared.completed"]}
    }
  ],
  "expires_at": null
}
```

Use a future RFC3339 timestamp for an expiry, or omit it for no automatic expiry. Requests require 1 to 256 fixed grants. Human-relative conditions such as `owner: self` are not transferred to a system runtime principal. Resolve those conditions into explicit resource scope before requesting approval.

Replacement is an explicit revocation, not an in-place extension. Existing policies and runs keep the predecessor ID and do not automatically gain the replacement's rights. To avoid stopping old runs during a planned transition, issue a separate approval, readmit future deployments under that ID, and drain old users before revoking the predecessor. A revoked record cannot be replaced to reactivate it; issue a new approval explicitly.

## CLI and MCP

```bash
attune pack approval list example --output json
attune pack approval show example 42 --output json
attune pack approval issue example --file approval.json
cat approval.json | attune pack approval issue example --file - --dry-run
attune pack approval replace example 42 --file approval.json --yes
attune pack approval revoke example 42 --yes
```

`--dry-run` validates and prints the request locally. It does not contact Attune or prove that the server will authorize it. Replacement and revocation require `--yes`; the CLI never waits for an interactive confirmation. Revocation is idempotent. Issuance creates a new record, so inspect the list before retrying after an ambiguous network failure.

MCP exposes `pack_approvals_list`, `pack_approvals_get`, `pack_approvals_issue`, `pack_approvals_replace` and `pack_approvals_revoke`. Replacement/revocation require `confirm_revocation: true`. All authorization remains server-side.

The web pack detail page links to External approvals. It displays fixed scopes and historical issuer attribution, supports pagination and issue/replace controls, and requires confirmation before explicit revocation.

## Ordering and audit

Runtime approval consumers hold shared approval-row locks through the protected transaction. Revocation waits for earlier admitted effects. A revocation committed before consumption is checked denies that effect. Attune does not retroactively undo a write admitted before revocation.

Successful controls emit audit events with subject pack and approval IDs. Audit details do not include tokens, key contents, or submitted grant bodies. Approval records and runtime admissions retain the attribution needed for later inspection, independently of the issuing human's account.
