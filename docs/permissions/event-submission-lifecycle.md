# Event submission authorization lifecycle

Source review of pack deployment, permission assignment, execution, and the direct event API, 2026-10-13. Paths and line numbers refer to the source inspected on that date. Tests were read, not run. No infrastructure was started or runtime code changed.

## Implementation update, 2026-10-15

The source review below describes the original human-delegated worker path.
The managed-pack implementation now has a separate, tested recorded-credential
path. It checks selected Events/Create against the locked actual trigger,
execution liveness, sponsor, admission, attempt and explicit revocation. Human
requester or approval-issuer inactivity is not a consumption dependency. The
production worker has not switched to these credentials; do not treat the old
event gate described below as globally fixed.

`GET /api/v1/events/{id}` can return `execution_emitter` for recorded emissions.
It contains immutable execution, admission, sponsor, pack, action, policy,
release and attempt identifiers. It is descriptive metadata, not a reusable
credential. Event read authorization runs first. Access-token callers also need
visibility of the emitting execution; sensor and execution tokens do not receive
this inspection projection. After execution retention, unrestricted execution
read authority is required because scoped workflow ancestry cannot be recovered
from emitter attribution alone. Requester and queue-item attribution are not
included. Existing sensor `source` and `source_ref` fields retain their meaning.
The web event detail uses the same filtered response.

See [implementation progress](../plans/self-contained-pack-implementation-progress.md)
for remaining production admission and consumer work.

## Main finding

`POST /api/v1/events` authorizes direct submission by JWT token type. Only `Sensor` and `Execution` tokens pass. The execution branch does **not** require an Events/Create grant, resolve the JWT's permission-set refs, check the current executor identity, or enforce an action-to-trigger scope. Sensor submission has additional trigger ownership, workload lease, generation, and membership checks. See `crates/api/src/routes/events.rs:93-95`, `:129-205`, `:264-336`, and `:405-454`.

This distinction matters for revocation. Named execution grants are checked against current identity authority on routes that invoke `AuthorizationService`. Direct event submission does not invoke that service. Removing those grants or freezing the executor therefore does not, by itself, revoke a previously issued execution JWT's direct event submission ability. This is a source-level conclusion, not a reproduced API test. See `crates/api/src/authz.rs:573-605` and the complete event handler at `crates/api/src/routes/events.rs:129-510`.

## End-to-end map

```text
Operator installs platform catalog -> core permission definitions in PostgreSQL
Pack author defines permission_sets/*.yaml and execution refs
  -> upload/register/install stages pack bytes
  -> verify installer can publish definitions AND delegate requested grants
  -> candidate tests
  -> activation transaction rechecks authority before mutations
       -> upsert permission sets before actions/rules/queues
       -> activate release and reconcile omitted components
       -> assign deployed rules' named execution sets to pack runtime identity
       -> capture rule input authority separately
       -> commit

IAM administrator assigns sets directly to identities or through roles
  -> execution creation chooses explicit refs or action defaults
  -> verify named grants are covered by attributed executor/caller
  -> store executor, selected refs, and pinned executable snapshot
  -> worker sees nonempty refs and issues Execution JWT
  -> inject ATTUNE_API_TOKEN into action process
       -> ordinary grant-checked API call
            resolve live selected sets + current executor coverage + route scope
       -> POST /api/v1/events
            validate JWT + require Execution token type + validate trigger/payload
            NO selected-set, active-executor, or execution-status check
  -> event persisted and EventCreated published
  -> matching rules execute under their own attributed runtime identities
```

The sections below cite the source for each step. The Compose database-loader path differs from API registration, as described under bootstrap.

## Permission names and separate mechanisms

The canonical RBAC representation uses the plural resource `events` and action `create`. `event:create` is not the literal resource name in this model. The schema permits an Events/Create grant, but the direct event route does not consult it. `event:emit` is not an RBAC action in the current source: the complete `Action` enum has no `Emit` member. See `crates/common/src/rbac.rs:11-52`.

Three different mechanisms should not be conflated:

| Mechanism | Current meaning |
| --- | --- |
| Events/Create RBAC grant, often described as `event:create` | A representable resource/action pair, not the authorization gate in `create_event`. |
| Execution `permission_set_refs` | Opt-in runtime token delivery and the named RBAC authority available to grant-checked routes. The worker embeds refs, not resolved grants. |
| Event emission using an execution JWT | Implicit permission from the `Execution` token type on the direct event route, not an `event:emit` grant. |

Sources: `crates/common/src/rbac.rs:13-52`; `crates/worker/src/executor.rs:129-151`, `:829-850`; `crates/common/src/auth/jwt.rs:595-610`; `crates/api/src/routes/events.rs:129-205`.

The reserved `standard` ref has no explicit Events/Create grant. Its generated grants cover action/pack-owned Keys read/decrypt, Artifacts read/create/update/delete, and read-only Caches. Current code includes cache access, beyond the shorter key/artifact description in AGENTS.md. Nevertheless, a runtime JWT minted because `standard` is present can pass the event route's execution-token gate. See `crates/api/src/authz.rs:797-857` and `crates/api/src/routes/events.rs:93-95`, `:129-205`.

| Execution configuration | Worker token delivery | Direct event submission with delivered token |
| --- | --- | --- |
| `permission_set_refs: []` | No token | No platform-provided credential to submit. |
| `permission_set_refs: [standard]` | Execution JWT | Allowed by token type despite no Events/Create grant. |
| Nonempty named refs, admitted at execution creation | Execution JWT | Allowed by token type, regardless of whether selected grants concern events. |
| User access token with Events/Create | Not an execution credential | Direct route rejects it; use the webhook ingress contract instead. |

Sources: `crates/worker/src/executor.rs:829-850`; `crates/api/src/routes/events.rs:134-205`. Normal payload/trigger validation still applies.

## Pack deployment and grant assignment

### Definition, assignment, and execution selection are different

| Declaration | Configuration | Effect |
| --- | --- | --- |
| Permission-set definition | Pack `permission_sets/*.yaml`, with `ref`, `grants`, optional label/description | Defines operations and scopes; does not automatically assign them to users. |
| Identity authority | `permission_assignment`, or role joins through `identity_role_assignment` and `permission_set_role_assignment` | Gives the actor effective grants from active sets. |
| Execution selection | Action `default_execution_permission_set_refs`; request/rule/queue/task `permission_set_refs` | Selects the authority requested by that run and opts into token delivery. |
| Rule input authority | Rule `input_permission_set_refs` | Captures Key/Pack read/decrypt authority for input rendering, separate from runtime API access. |

Sources: `crates/common/src/pack_registry/permissions.rs:34-119,181-207`; `crates/common/src/repositories/identity.rs:183-200`; `crates/common/src/repositories/rule_authority.rs:349-410`.

Grant objects contain `resource`, `actions`, and optional `constraints`. Constraints include component `refs`, `pack_refs`, ownership, execution scope, and attributes. Pack admission forbids database-ID constraints and combining `refs` with `pack_refs`. Set refs are read literally, not automatically prefixed with the pack name. Sources: `crates/common/src/rbac.rs:72-104`; `crates/common/src/pack_registry/permissions.rs:34-50,381-408`.

### Registration checks and persistence

The API checks permission metadata before candidate tests and again inside activation. It captures the installer's active authority before changing permission sets. The transactional component loader also validates the plan before mutations. An installer therefore cannot broaden its assigned set and use the replacement to authorize the same deployment. Sources: `crates/api/src/routes/packs.rs:2095-2117,2322-2345`; `crates/common/src/pack_registry/loader.rs:315-335`.

Publishing a definition requires either coverage of its grants or unrestricted `permissions:manage`. Attaching named execution refs still requires coverage of their grants. Permission-management authority is not a runtime delegation bypass. External refs must resolve to active sets. Coverage compares effective resource/action/scope authority, not possession of the identical set ref. Workflow-task expressions are deferred to rendered child creation. Sources: `crates/common/src/pack_registry/permissions.rs:211-318,345-378`; `crates/common/src/delegation.rs:71-94,100-129`.

The loader persists sets first, before dependent actions, queues, and rules. Grants are JSON metadata in PostgreSQL, associated with their managing pack. Redeployment checks ownership, updates existing rows in place, preserves IDs, and reactivates incoming retired sets. Sources: `crates/common/src/pack_registry/loader.rs:368-390,689-859`.

API activation attributes the installer through `installed_by`, activates the release, captures rule authority, and commits together. Capture creates or reuses a passwordless system-owned pack identity, assigns rules' selected named execution sets to it, and sets `rule.owner_identity` to that identity. `standard` is excluded from database set assignment. The installer is not the deployed rule's runtime executor. Sources: `crates/api/src/routes/packs.rs:2519-2554`; `crates/common/src/repositories/rule_authority.rs:310-343,371-424`.

Rule input grants are separately captured for server-rendered Key/Pack inputs. They do not become action API grants or control event emission. Sources: `crates/common/src/repositories/rule_authority.rs:349-370,401-410`; `docs/permissions/delegation-and-secret-disclosure.md:65-74`.

### Administrative assignment and editing

Direct identity assignments, role membership, and set-to-role mappings are separate IAM operations protected by `permissions:manage`. Effective identity grants expand through both direct assignments and role mappings, excluding retired sets. Existing non-platform sets can be edited through the permissions API; platform-managed definitions cannot. Changes invalidate caches and publish metadata-change messages. Sources: `crates/api/src/routes/permissions.rs:56-109,462-515,626-650,761-767,889-895`; `crates/common/src/repositories/identity.rs:183-200`.

Available CLI commands include the following. These examples were not executed.

```sh
attune permission set show example.runtime_access
attune permission assign example.runtime_access --identity alice@example.com
attune permission assign example.runtime_access --role automation-operators
attune permission set update example.runtime_access --file runtime_access.yaml --dry-run
attune permission revoke example.runtime_access --identity alice@example.com
```

Source: `crates/cli/src/commands/permission.rs:16-107`.

For a new named policy, an administrator can deploy the definition without attaching it, assign sufficient authority to the registrar/executor, and then deploy refs requesting it. `core.admin` grants Events read/decrypt, not create. An explicit Events/Create policy is representable and subject to the same deployment checks, but it is unnecessary for direct execution-token event emission today. Sources: `crates/common/src/platform_catalog/permission_sets/admin.yaml:13,24`; `crates/common/src/pack_registry/permissions.rs:216-222,317`; `crates/api/src/routes/events.rs:134-205`.

### Platform catalog and bootstrap

`core.admin`, `core.editor`, `core.executor`, and `core.viewer` are compiled platform-catalog definitions. API startup reconciles them into PostgreSQL independently of filesystem pack installation. Matching old core-pack metadata is treated as platform metadata rather than replacing platform-owned sets. Sources: `crates/common/src/platform_catalog.rs:70-87`; `crates/common/src/repositories/platform_catalog.rs:153-185,234-243`; `crates/api/src/main.rs:382-387`; `crates/common/src/pack_registry/loader.rs:739-745`.

HTTP core bootstrap authenticates an existing deployment credential supplied through `ATTUNE_BOOTSTRAP_TOKEN` and uploads through the ordinary API admission path. It does not create identities, assign `core.admin`, or open a database. See [bootstrap admission](../deployment/candidate-api-effects.md#bootstrap-through-the-api).

Default local Compose now uses that API publisher too. Volume initialization assembles a private candidate rather than overwriting active metadata, and the former direct Python loader is an API-only wrapper. Legacy release scanning no longer activates unadmitted content. Bootstrap verifies an exact current core runtime-policy/release binding; platform readiness remains available before content admission. See [candidate effects and bootstrap admission](../deployment/candidate-api-effects.md).

### Redeployment and removal

- Named grants remain mutable authorization metadata even when executable releases are immutable. Both registration and IAM edits can replace their grants. A later deployment can overwrite an API edit. Sources: `crates/common/src/pack_registry/loader.rs:793-833`; `crates/api/src/routes/permissions.rs:504-515`.
- Omitting a set YAML file does not always revoke it. Deployment policy `remove` retires omitted sets; `disable` and `retain` leave these non-toggleable sets unchanged. Active-set queries exclude retired rows. Sources: `crates/common/src/repositories/component_lifecycle.rs:236-320`; `crates/common/src/repositories/identity.rs:190-198`.
- Rule execution-set assignments are additive in deployment capture and refresh. Those methods insert missing assignments but do not prune older ones. Removing a rule's requested ref does not itself revoke that set from the shared pack identity, although selected execution refs remain a separate restriction on grant-checked routes. Sources: `crates/common/src/repositories/rule_authority.rs:117-169,378-400`.

Deployment tests cover undelegable definitions, admin refs attached through action/rule/queue metadata, installer versus rule-owner separation, and explicit empty refs overriding action defaults. They were inspected, not run. Source: `crates/api/tests/pack_registry_tests.rs:83-187`.

## Execution creation and snapshots

### Manual execution

The execution API resolves the server-owned immutable executable snapshot in the creation transaction. It chooses requested `permission_set_refs` when supplied, otherwise the selected action's default refs. Before creation, it asks whether the requesting token's effective grants cover the named requested sets. An explicit empty list overrides nonempty action defaults. See `crates/api/src/routes/executions.rs:194-223`.

`can_delegate_permission_sets_with_snapshot` strips the reserved `standard` ref from named-policy resolution, resolves requested sets, rejects missing sets, and compares their grants with the caller's current effective grants. For execution-token callers, those caller grants come from embedded refs, not unrestricted identity roles. See `crates/api/src/authz.rs:395-437`, `:573-605`, `:732-740`, `:770-778`.

The execution row stores `executor`, `permission_set_refs`, release ID/digest, and the executable snapshot separately. The executable snapshot pins code and contracts. It is not a frozen copy of the named permission sets used by the API's execution-token grant resolver. See `crates/common/src/repositories/execution.rs:319-354` and `crates/api/src/authz.rs:580-603`.

### Rule-created execution

The enforcement processor requires an attributed execution identity and loads its active delegation authority. It uses the enforcement's immutable executable snapshot and selects captured execution refs when the input snapshot supplies them, otherwise rule refs or action defaults. It checks the named refs under a shared lock before storing `executor` and `permission_set_refs` on the execution. See `crates/executor/src/enforcement_processor.rs:350-393`, `:411-419`.

The identity that later emits an event is not automatically the identity under which matching rules execute. Rule-created execution authority comes from the enforcement/rule owner path above. The event route stores sensor source fields only; its execution-token branch returns no source ID/ref. See `crates/api/src/routes/events.rs:173-205`, `:392-403`.

## Runtime JWT minting and delivery

The worker reads the stored execution refs. Empty refs mean no `ATTUNE_API_TOKEN`. Nonempty refs require an explicit `execution.executor`; there is no identity-1 fallback in the implementation. The nearby comment still describes such a fallback and is stale. See `crates/worker/src/executor.rs:117-151`, `:816-850`.

The worker mints the JWT with a TTL equal to the resolved execution timeout plus 60 seconds. It puts the token in the child environment as `ATTUNE_API_TOKEN`. The standalone token generator defaults to 360 seconds when no TTL is passed, but the worker supplies its calculated TTL. Minting failure logs a warning and leaves the action without a token. See `crates/worker/src/executor.rs:835-860` and `crates/common/src/auth/jwt.rs:576-587`.

The JWT carries:

- `sub` equal to the attributed executor identity ID.
- `login` equal to `execution:<execution_id>`.
- `token_type = Execution`, `scope = execution`, `iat`, and `exp`.
- Metadata containing execution ID, action ref, permission-set refs, and standard-access action/pack refs.

The generator signs with `Header::default()` and the configured shared secret. Validation explicitly permits HS256. It does not embed resolved named grants, an execution revocation generation, or a per-token ID in these claims. See `crates/common/src/auth/jwt.rs:595-638`.

For workflow-task executions, standard-access owner refs include the child's action and its immediate parent's action. Pack refs are derived from those action refs. Missing parents produce a warning and do not grant that parent scope. This broadens scoped standard resource access, not event authorization. See `crates/worker/src/executor.rs:1074-1096` and `crates/common/src/auth/jwt.rs:589-601`.

SDK clients consume the supplied token; they do not assign permissions. The Python client reads `ATTUNE_API_URL` and `ATTUNE_API_TOKEN`, and its generated event call sends JSON to `/api/v1/events`. Action SDK `emit_result` only writes JSON to stdout, not an event. Sources in the sibling SDK repository: `../python-attune-sdk/src/attune/client.py:32-78`; `../python-attune-sdk/src/attune/api_client/api/events/create_event.py:13-29,77-103`; `../python-attune-sdk/src/attune/action.py:41-43`.

## JWT validation versus live grant resolution

`RequireAuth` obtains validated claims and permits Access, Sensor, Execution, and Worker token types. It performs a database-backed workload fence check for Sensor tokens only. It does not load an execution row, compare JWT metadata with stored refs, require Running status, or check the execution subject's active identity. See `crates/api/src/auth/middleware.rs:121-170`.

Bearer validation calls `validate_token`, which uses the configured secret and `jsonwebtoken::Validation::default()` with an HS256-only algorithm list. Expiry behavior, including the library's default tolerance, is inherited from that validation configuration. No execution-specific revocation lookup appears in this path. See `crates/api/src/auth/middleware.rs:173-180` and `crates/common/src/auth/jwt.rs:621-638`.

For routes that ask for effective execution grants, the API instead does all of the following:

1. Extract named refs from token metadata, excluding `standard`.
2. Resolve active permission sets directly through the repository and reject unavailable refs.
3. Parse their current grants.
4. Load the subject's current, unfrozen delegation authority and require coverage of every named grant.
5. Append the synthetic standard-access grants.

See `crates/api/src/authz.rs:573-605`, `:732-778`, `:797-857`; `crates/common/src/delegation.rs:37-49`, `:71-73`, `:124-145`. The active-set query filters `retired_at IS NULL`, so retired/deleted refs fail resolution. See `crates/common/src/repositories/identity.rs:945-962`.

These are live grants, not immutable token-time grants. Permission-set edits can change the authority of an existing execution token, but only while the current executor still covers the resulting grants. Retiring a referenced set, removing sufficient identity authority, or freezing/removing the identity makes this grant-resolution path fail. The execution branch deliberately uses a direct repository read rather than its permission-set lookup cache. See `crates/api/src/authz.rs:580-603` and `crates/common/src/delegation.rs:37-49`, `:124-130`.

That revocation boundary applies only when the consuming route invokes it. Direct event creation invokes neither `load_snapshot` nor `effective_grants`. Event reads do invoke effective grant resolution, a separate contract from submission. See `crates/api/src/routes/events.rs:129-510`, `:530-531`, `:654-660`.

## Direct event route sequence

`events::routes` wires POST `/events` to `create_event`; the handler uses `RequireAuth`. The server merges these routes under `/api/v1`. Its outer layers add tracing, CORS, logging, and auditing, not a separate global Events/Create check. See `crates/api/src/routes/events.rs:129-133`, `:1494-1497`; `crates/api/src/server.rs:48-81`, `:89-120`.

After authentication, the handler:

1. Rejects all token types except Sensor and Execution with 403. User-session and integration access tokens cannot bypass this by holding Events/Create. The error directs users to the trigger webhook. See `crates/api/src/routes/events.rs:134-147`.
2. For sensors only, requires an exact trigger ref in JWT `trigger_types`. See `:149-157`.
3. Resolves the trigger and rejects unknown, disabled, or retired triggers. See `:159-170`.
4. For sensors, resolves the sensor from JWT login, checks that it owns the trigger, and requires a workload fence. For execution JWTs, this branch performs no trigger-scope or grant check. See `:172-205`.
5. Parses optional `trigger_instance_id` as `rule_<positive numeric id>`. An execution caller's targeted rule must exist and reference the chosen trigger, but there is no rule-read, rule-execute, or owner authorization check here. Sensors receive later workload membership checks. See `:207-244`, `:430-452`.
6. Accepts an explicit normalized trace tag or inherits the execution's trace tag. Missing execution rows yield no inherited tag, not rejection. That lookup is correlation, not validation of execution authority or status. See `:246-262`.
7. Selects the current event schema for execution callers. Sensors instead use their admitted generation's trigger binding. It prepares/validates payload and config, encrypts secret values, creates the event, attaches the binding, and persists secret values in one transaction. See `:264-337`, `:387-474`.
8. Publishes EventCreated after commit if a publisher exists. Publication failure warns but does not roll back the accepted event. Returns 201. See `:476-509`.

Consequently, the execution branch has no pack-local trigger restriction. Any enabled, non-retired trigger that accepts the submitted schema can be selected, including a trigger owned by a sensor. Schema validation and a valid rule linkage are data constraints, not proof that the caller was delegated emission rights. See `crates/api/src/routes/events.rs:159-205`, `:217-244`, `:334-337`.

## Workflow execution

Workflow tasks do not automatically copy their parent's permission refs. Absent a task override, the scheduler uses the task action's default refs. A task override is rendered from workflow context, then normalized from null, string, or string array. Empty refs mean no token for that child. See `crates/executor/src/scheduler.rs:2321-2380` and `crates/worker/src/executor.rs:129-151`.

Ordinary child creation verifies the rendered refs against `parent_execution.executor`, then stores that identity and those selected refs in the child. Transactional task creation, batch, item, and cache-iteration child paths also call the shared delegation check. See `crates/executor/src/scheduler.rs:2928-2974`, `:3933-3941`, `:4638-4646`, `:4930-4938`, `:5197-5205`. The shared helper requires an explicit active identity for any nonempty refs, including `standard`, while named refs additionally require grant coverage. See `crates/common/src/delegation.rs:167-203`, `:100-122`, `:134-145`.

Cache iteration also limits named cache permission refs to those delegated by the parent before authorizing the read. This is a distinct cache-specific restriction, not an event submission scope. See `crates/executor/src/scheduler.rs:4290-4323`.

A workflow child with a token follows the same event route as any other execution. Neither its workflow lineage nor its selected permission refs are checked for direct emission. A parent workflow's token opt-in is therefore not enough to give every child a token, but any child that does receive an execution token can pass the token-type emission gate. See the child selection/minting sources above and `crates/api/src/routes/events.rs:134-205`.

## Sensor comparison

Sensor token creation resolves registered sensor authority inside a transaction, locks the current workload fence, checks an existing sensor identity is not frozen, and mints a token containing admitted trigger types, cache authority, and workload fence. The default TTL here is 24 hours. Sensor process startup injects that returned token as `ATTUNE_API_TOKEN`. See `crates/api/src/routes/auth.rs:1490-1572` and `crates/sensor/src/sensor_manager.rs:1534`.

Sensor request authentication checks workload ID, worker ID, worker instance UUID, generation, and unexpired assignment lease. The normal sensor-scoped query also checks matching sensor/pack refs, effective enablement, and non-retirement. See `crates/api/src/auth/middleware.rs:147-167` and `crates/common/src/repositories/sensor_workload.rs:271-318`.

The event handler then checks exact trigger scope and ownership. Inside its transaction it locks the assignment, requires the retained sensor generation and matching generation owner, and requires an admitted trigger binding. Before insertion it checks the fence again and verifies effective sensor/trigger membership. Targeted rules must also be enabled, non-retired workload members using that trigger. See `crates/api/src/routes/events.rs:149-201`, `:264-293`, `:324-333`, `:405-452`; `crates/common/src/repositories/sensor_workload.rs:353-424`.

This gives sensor tokens live lease/generation revocation that execution event submission lacks. Sensor emission is not authorized through identity RBAC or Events/Create either. Its separate authority is the admitted workload and trigger contract. See `crates/api/src/authz.rs:294-307` and the sensor checks above.

## Test evidence and gaps

Inspected evidence:

- Token-type unit test explicitly accepts Sensor/Execution and rejects Access/Worker/Refresh. Exact positive rule IDs and exact sensor trigger matching have nearby unit tests. These do not test the full HTTP authorization path. `crates/api/src/routes/events.rs:1508-1536`.
- Worker unit tests cover no runtime token without permission refs and JWT metadata with explicit refs and standard owner context. `crates/worker/src/executor.rs:2432-2490`.
- API integration tests show empty execution-token refs deny runtime reads and action execution, read-only refs do not permit execution, and broader delegation fails. They establish grant-checked route behavior, not event submission behavior. `crates/api/tests/execution_token_permissions_e2e.rs:503-628`.
- A key API test verifies covered named-token access succeeds, an uncovered set fails with 403, and a frozen executor fails with 401. `crates/api/tests/key_usage_tests.rs:675-744`.
- Scheduler tests reject rendered permission escalation for both single and item fanout tasks, create no children after denial, then permit a child after assigning sufficient identity authority. `crates/executor/src/scheduler.rs:9217-9298`.
- A sensor integration test freezes the installer and clears its permission-set grants, then still expects sensor event submission to return 201 using an admitted workload token. This supports the sensor release-owned runtime contract, not an execution JWT revocation contract. `crates/api/tests/key_usage_tests.rs:182-215`.

Potential gaps worth testing or deciding explicitly:

1. Execution-token event submission bypasses current named grant resolution and active-owner checks. A leaked token appears usable for submission until JWT validation rejects it, even if its permission sets are retired or its executor is frozen. This follows from the handler and extractor, but no direct HTTP regression test for these cases was found in the inspected API tests. `crates/api/src/auth/middleware.rs:121-170`; `crates/api/src/routes/events.rs:129-510`.
2. Token opt-in through `standard` or an unrelated named permission set implicitly enables event submission across trigger packs. If emission should be least-privilege, it needs a deliberate grant/scope contract and handler enforcement; neither an Events/Create check nor `event:emit` currently supplies one. `crates/worker/src/executor.rs:829-850`; `crates/api/src/routes/events.rs:134-205`; `crates/common/src/rbac.rs:13-52`.
3. A valid execution JWT does not need a retained, running execution row for submission. The optional trace-tag lookup is the only execution-row lookup in this handler. Completion, cancellation, or execution retention is not checked as token revocation. `crates/api/src/routes/events.rs:246-262`; `crates/api/src/auth/middleware.rs:121-170`.
4. Execution events store no execution source ID/ref. An inherited trace tag is useful correlation but is overridable and does not establish emitter lineage. `crates/api/src/routes/events.rs:173-205`, `:246-262`, `:392-403`.
5. The worker's identity-1 fallback comment contradicts its fail-closed identity resolver. Documentation or review based only on that comment would reach the wrong conclusion. `crates/worker/src/executor.rs:117-127`, `:823-835`.
6. Compose's direct database bootstrap and API registration establish different permission metadata and rule ownership. Release freezing does not close that difference. Validate this path before assuming a pack deployed through Compose has the same execution refs and captured rule authority as one uploaded through the API. Sources are in the bootstrap section above.

These gaps describe the current source contract. Whether token-type-based execution emission is intentional or should be replaced by scoped emission grants needs a policy decision. No claim here relies on a successful test run.
