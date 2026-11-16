# Identity profile and administration authority

The canonical v1 identity API distinguishes profile editing from sensitive
administration. Existing global `identities:update` grants now authorize only
`display_name` edits. They do not authorize password resets, integration-token
creation/revocation/deletion, freeze/unfreeze, or authorization/provider attributes.

Explicit `identities:manage` authorizes freeze/unfreeze and integration-token
revocation/deletion, including on self. Password resets, integration-token
creation, and arbitrary attribute writes additionally require `permissions:manage`.
This extra gate applies even to self, unchanged attributes, and apparently
unprivileged targets: credentials inherit the target's present and future grants,
and attributes affect RBAC predicates and external account bindings. Identity
creation with nonempty attributes requires `permissions:manage` too. No privilege
ceiling is inferred from current role membership. Both checks use the supplied
credential, so a narrow execution token cannot borrow its owner's IAM authority.
The platform `core.admin` already holds both permissions; no catalog update is
needed. Safe display-name edits retain `identities:update`; scoped identity
management remains required even for permission administrators. Local password
writes are rejected for OIDC/LDAP identities before any field is changed.
Role/permission assignments and external identity mappings also require
`permissions:manage`.
Mapping creation and deletion require that gate on the integration and mapped
identity; rebinding requires it on both the old and new mapped identities too.
Mapping mutation scope resolves persisted identity logins (`refs`) and ownership
(`owner: self`), and the supplied credential must cover every affected target.
Read/list continue to require `identities:read` on the integration. A profile
editor cannot bind an external subject to an inquiry assignee; a legitimately
admitted callback integration still resolves administrator-configured mappings.

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
