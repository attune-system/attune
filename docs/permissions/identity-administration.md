# Identity profile and administration authority

The canonical v1 identity API distinguishes profile editing from sensitive
administration. Existing global `identities:update` grants now authorize only
`display_name` edits. They do not authorize password resets, integration-token
creation/revocation/deletion, freeze/unfreeze, or authorization/provider attributes.

Explicit `identities:manage` authorizes those sensitive operations. This includes
administration of the caller's own identity; self access is never implicit.
Grant this permission only to trusted identity administrators: resetting a password
or minting a token can impersonate the target with its effective permissions.
The platform `core.admin` catalog includes it. Custom administrators must add it
explicitly; profile editors must not receive it. Role and permission assignments
and external identity mappings retain their separate `permissions:manage` gate.

PUT requests require authority for every supplied non-null field. Mixed profile
and sensitive updates require both capabilities, checked before any write. Send
only fields being edited; do not echo attributes, passwords, or frozen state in
a profile form. The UI currently has no full-form profile update consumer; its
update hook passes the caller's explicit partial request through unchanged.
No stale value comparison grants an exemption for echoed sensitive fields.

Single-identity read, update, delete, token, and freeze checks resolve target ID,
login ref, and identity ownership from the persisted identity. Scope grants can
use `refs` (login) and `owner: self`. Database IDs are resolved internally;
authored permission sets continue to reject ID selectors. An owner-self grant cannot
administer another identity. List/create retain collection-level authorization.
Missing scope context previously generally denied scoped grants; it was not a
proven scoped fail-open. Global update's credential authority was the separation
risk. Attribute/freeze changes invalidate authorization metadata. Password session
revocation is owned by the interactive-session contract.

Catalog revision 4 (compatibility epoch 1) reconciles the explicit administration
grant into installed platform-owned `core.admin` rows at API startup. Catalog IDs,
direct assignments, and role links remain intact. Current-revision reconciliation
remains a no-op for stored component content; its bootstrap source snapshot stays
consistent. Custom profile/editor grants gain no administration permission.
Both shipped admin YAML definitions intentionally include the new action; all
other platform definition bytes remain unchanged.

The bundled core version for this policy change is `1.0.6`, independently of
platform/API `0.10.4`. Its existing action, trigger, sensor, and runtime inventory
is unchanged.
