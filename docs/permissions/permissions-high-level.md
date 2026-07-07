# Authorization Model (Preamble)
- Authorization has no explicit deny grants; however, access is not a simple union of all matching grants. Within a single applicable visibility path, matching grants are additive (union). Across paths, resources fall into two classes:
  - Additive-path resources: access is the union of all authorized paths (for example, inquiries)
  - Authoritative-path resources: a designated derived-visibility path is exclusive and fail-closed (for example, execution-linkage for artifacts; rule-derived visibility for events/enforcements). When that path applies, other non-global grants MUST NOT broaden access, and if that path resolves to no readable target the row is denied
  - Un-scoped (Global) read is the only path that always overrides both classes
- "Un-scoped (Global) read" means an unconstrained read grant on the specific resource type being evaluated (it does not imply global read across all resource types)
- For resources using restricted reference visibility, a caller with pack-scoped read is evaluated as a valid referencing pack for that pack; the owning pack is always valid
- This document defines read/list/search/download/stream visibility semantics; create/update/delete/lifecycle authorization is governed separately by action-level grants
- Default visibility is fail-closed: if a resource visibility value is unset, null, unrecognized, or indeterminate, it MUST be treated as private/scoped (never public/no-filter)
- Grant/evaluation vocabulary applies throughout:
  - "Pack-scoped read" means a scope grant naming the owning pack
  - "Specific-resource read" means a scope grant naming the individual target resource
  - "Resource scope check" succeeds when the caller holds a pack-scoped or specific-resource read grant for that resource
  - "Visibility model" means the resource's own public/private/restricted classification, evaluated after applicable scope checks
  - "Un-scoped (Global) resource read" means an unconstrained read grant on that resource type; it overrides scope checks, visibility model checks, and parent-resource visibility for derived resources as an intentional admin-level override

# Actions
- All authenticated users should be allowed to list/search for actions
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) action read should grant access for all actions without filters, regardless of other grants
  - Evaluation order should be: Un-scoped (Global) read -> action scope check -> action visibility model
  - Actions configured as public should be visible to any authenticated user
  - Actions configured as private should only be visible to users with permissions granting access within the relevant scope (pack or specific action)
  - Actions configured as restricted with other packs specified should be visible only when the caller has pack-scoped read on the owning pack or at least one allowlisted pack.
  - Visibility boundary: if a private workflow references public actions, workflow-level private metadata MUST NOT be exposed through action list/search results, expansions, or related projections. Public action visibility does not grant visibility into private workflow context.

# Executions
- All authenticated users should be allowed to list/search for executions
- The row-level results will be restricted in the following manner:
  - Un-scoped (Global) execution read should grant access regardless of any other grants
  - Evaluation order should be: Un-scoped (Global) read -> execution ownership/ancestor scope -> parent inheritance -> explicit execution-scoped grants
  - Execution ownership and descendant visibility checks should use materialized ancestry/ownership keys in database filters (not recursive per-request tree walks)
  - Parent inheritance is authoritative: executions with a parent id are evaluated from their nearest controlling ancestor/root workflow execution, and child visibility MUST NOT broaden access beyond that parent chain
  - Action reference visibility (public/private/restricted) governs action discoverability and cross-pack referencing, and MUST NOT by itself grant execution-row visibility
  - Execution-row visibility must be derived from execution ownership/ancestry and execution-scoped grants, not from action publicity alone
  - Non-leak boundary for subtasks: execution rows for public child actions inside private workflows are not independently discoverable; they inherit private parent/workflow accessibility
  - List/search boundaries: execution list/search results, expansions, and aggregates MUST NOT expose private parent workflow refs, names, or topology unless the caller can read that parent execution

# Rules
- Rules have no public/private visibility toggle; they are treated as private-scoped metadata by default
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) rule read should grant access for all rules without filters, regardless of other grants
  - Evaluation order should be: Un-scoped (Global) read -> rule scope check -> deny
  - Callers must hold explicit read authority (Un-scoped (Global) rule read, pack-scoped rule read, or specific-rule read) for rules to be returned
  - Rule-linked action/trigger metadata follows the same private-scoped assumption and must not be returned from rule datasets unless the caller satisfies the rule read scope
  - Rules should only be visible to users with permissions granting access within the relevant scope (pack or specific rule)
  - List/search boundaries: rule list/search results, expansions, and aggregates MUST NOT expose private trigger/event linkage details unless the caller can read those linked resources

# Enforcements
- Authenticated users may call list/search endpoints for enforcements, but row return is strictly scope-gated
- The row-level results will be restricted in the following manner:
  - Un-scoped (Global) enforcement read should grant access regardless of any other grants
  - Evaluation order should be: Un-scoped (Global) read -> rule-derived visibility
  - Because rules are private-scoped metadata, non-global enforcement visibility requires explicit rule read scope (pack-scoped or specific-rule read) on the originating rule
  - Enforcements for rules should only be visible to users with permissions granting access within the relevant scope (pack or specific rule)
  - Rule-derived visibility is authoritative for all non-global paths: aside from Un-scoped (Global) enforcement read, enforcement visibility MUST NOT be broader than visibility of its originating rule
  - Enforcements have no alternate trigger-derived or event-derived fallback path; if rule visibility does not grant access, the row is denied
  - Visibility evaluation for enforcements is dynamic: access is determined from the current visibility/permissions of the originating rule at read time (not snapshotted at enforcement creation time)
  - List/search boundaries: enforcement list/search results, expansions, and aggregates MUST NOT expose private event payloads, trigger context, or execution linkage unless the caller can read those linked resources

# Triggers
- All authenticated users should be allowed to list/search for triggers
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) trigger read should grant access for all triggers without filters, regardless of other grants
  - Evaluation order should be: Un-scoped (Global) read -> trigger visibility model
  - Triggers configured as public should be visible to any authenticated user
  - Triggers configured as private should only be visible to users with permissions granting access within the relevant scope (pack or specific trigger)
  - Triggers configured as restricted with other packs specified should be visible only when the caller has pack-scoped read on the owning pack or at least one allowlisted pack
  - List/search boundaries: trigger list/search results, expansions, and aggregates MUST NOT expose private rule relationships or event-derived metadata unless the caller can read those linked resources

# Events
- All authenticated users should be allowed to list/search for events
- The row-level results will be restricted in the following manner:
  - Un-scoped (Global) event read should grant access regardless of any other grants
  - Evaluation order should be: Un-scoped (Global) read -> rule-derived visibility (if event has a rule association record) -> trigger-derived visibility (only if no rule association record exists)
  - Events not associated with a specific rule:
    - Events for triggers configured as public should be visible to any authenticated user
    - Events for triggers configured as private should only be visible to users with permissions granting access to triggers within the relevant scope (pack or specific trigger)
    - Events for triggers configured as restricted with other packs specified should be visible only when the caller has pack-scoped read on the owning pack or at least one allowlisted pack
  - Events associated with a specific rule should derive their visibility from the rule they are published for:
    - Rule-derived visibility is authoritative and MUST NOT be broadened by trigger publicity
    - Rule association used for visibility must be system-generated, validated against the rule/trigger relationship, and immutable to callers
    - Caller-supplied event fields (including correlation/metadata fields) MUST NOT influence rule-association visibility selection
    - Events for rules should only be visible to users with permissions granting access to rules within the relevant scope (pack or specific rule)
    - Visibility evaluation is dynamic: access is determined from the current visibility/permissions of the associated rule at read time (not snapshotted at event ingest time)
  - List/search boundaries: event list/search results, expansions, and aggregates MUST NOT expose private payload fields, rule refs, trigger refs, or linked execution context unless the caller can read those linked resources

# Artifacts (Suggested Implementation)
- All authenticated users should be allowed to list/search for artifacts
- Row-level results are evaluated with execution-derived visibility taking precedence over owner-derived visibility:
  - If one or more execution-linkage records exist (via artifact version records), access is granted only when the caller can read at least one linked execution; owner-path visibility must not broaden access
  - If no execution-linkage records exist, access is determined solely by owner-path visibility
- Owner path is evaluated from artifact owner scope (`identity`, `pack`, `action`, `sensor`) and visibility (`public` vs `private`)
- The final read decision should be:
  - Un-scoped (Global) artifact read is evaluated first and grants access regardless of execution-linkage or owner-path evaluation
  - For artifacts with execution linkage, execution-derived visibility is authoritative and non-global owner-path visibility MUST NOT broaden access
  - For artifacts without execution linkage, owner-path visibility determines access
  - **Deny** when no applicable path grants access
- Owner-path behavior should follow these rules:
  - Un-scoped (Global) artifact read should grant access regardless of other grants
  - Public artifacts should be visible to any authenticated user
  - Private artifacts should require scoped access to the owning scope (or inherited pack-scoped access for action/sensor-owned artifacts)
  - Identity-owned private artifacts should be visible only to the owning identity unless explicit scoped grants allow otherwise
- Execution-path behavior should follow execution visibility semantics:
  - If a linked execution is a child execution, visibility is determined by the parent execution’s accessibility
  - This inheritance must take precedence over child action visibility. A child execution of a public action does **not** become broadly readable when its parent workflow/action execution is private
  - Artifacts produced by public-action subtasks inside private workflows (for example stdout/stderr artifacts) should therefore be restricted to identities that can read the private parent workflow execution
  - If an artifact has versions from multiple executions, visibility is granted when any one linked execution is readable
  - If execution-linkage records exist but no linked execution resolves to a readable execution, access is denied and MUST NOT fall back to owner-path visibility
  - Version-level reads/downloads/streams MUST be evaluated per artifact version linkage; callers may access only versions whose linkage grants read access
  - "Latest" version endpoints for non-global callers MUST return the latest readable version (or deny when none is readable), never an unreadable latest version
  - Linkage used for execution-derived visibility must be system-generated and immutable by callers; caller-supplied linkage must never broaden visibility
- Write operations (`create`, `update`, `delete`) should remain owner-scoped and should not be granted by execution linkage alone

# Workflows (Suggested Implementation)
- All authenticated users should be allowed to list/search for workflows
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) workflow read should grant access regardless of any other grants
  - Evaluation order should be: Un-scoped (Global) read -> workflow scope check
  - Workflows should only be visible to users with permissions granting access within the relevant scope (pack or specific workflow)
  - List/search boundaries: workflow list/search results, expansions, and aggregates MUST NOT expose private child task/action metadata unless the caller can read those linked resources

# Packs (Suggested Implementation)
- All authenticated users should be allowed to list/search for packs
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) pack read should grant access regardless of any other grants
  - Packs should only be visible to users with permissions granting access within the relevant scope (pack)
  - Pack-index/catalog entries should follow the same visibility model; indexed metadata MUST NOT reveal private pack configuration or private component refs

# Sensors (Suggested Implementation)
- All authenticated users should be allowed to list/search for sensors
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) sensor read should grant access regardless of any other grants
  - Sensors should only be visible to users with permissions granting access within the relevant scope (pack or specific sensor)
  - If sensor placement/worker metadata is included in responses, it MUST be filtered to avoid leaking infrastructure details to callers without worker/admin visibility

# Sensor Logs (Suggested Implementation)
- Sensor log read should be treated as sensitive read access (not public by default)
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) sensor-log read should grant access regardless of any other grants
  - Sensor log visibility should inherit sensor visibility; a caller must be able to read the parent sensor to read its logs
  - Log payload boundaries: responses MUST NOT expose private content fields (raw stdout/stderr chunks, secrets, env-derived data) beyond the caller’s sensor/log access scope

# Keys (Suggested Implementation)
- Key records should be owner-scoped resources (`system`, `identity`, `pack`, `action`, `sensor`)
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) key read should grant access regardless of any other grants
  - Key metadata visibility should require owner-scope access
  - Key material visibility/decryption should require explicit decrypt-level permission; metadata read MUST NOT imply value/decrypt access
  - Identity-owned keys should be visible only to the owning identity unless explicit scoped grants allow otherwise

# Work Queues (Suggested Implementation)
- All authenticated users should be allowed to list/search for queues
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) queue read should grant access regardless of any other grants
  - Queues should only be visible to users with permissions granting access within the relevant scope (pack or specific queue)
  - Queue item/execution linkage shown in queue responses MUST be filtered so linked execution visibility is never broadened by queue visibility

# Inquiries (Suggested Implementation)
- Inquiries should be visible only to identities that are authorized participants (requester, assigned responder, or authorized scope readers)
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) inquiry read should grant access regardless of any other grants
  - Inquiry prompt/response content visibility requires an authorized participant/scope-reader path; linked execution visibility alone is not sufficient to grant inquiry-content access
  - For participants/scope-readers, linked execution visibility controls whether linked-execution details are included; when linked execution is unreadable, linked-execution fields should be redacted rather than using unreadable linkage to deny participant access
  - Inquiry response payloads MUST follow non-leak boundaries for linked execution/artifact/event resources

# Policies (Suggested Implementation)
- All authenticated users should be allowed to list/search for policies
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) policy read should grant access regardless of any other grants
  - Policies should only be visible to users with permissions granting access within the relevant scope (pack or specific policy)
  - Policy evaluation details and private match criteria MUST NOT be exposed in list/search expansions to unauthorized callers

# Dashboards (Suggested Implementation)
- All authenticated users should be allowed to list/search for dashboards
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) dashboard read should grant access for all dashboards without filters, regardless of other grants
  - Evaluation order should be: Un-scoped (Global) read -> dashboard scope check -> dashboard visibility model
  - Dashboards configured as public should be visible to any authenticated user
  - Dashboards configured as private should only be visible to users with permissions granting access within the relevant scope (pack or specific dashboard)
  - Dashboards configured as restricted with other packs specified should be visible only when the caller has pack-scoped read on the owning pack or at least one allowlisted pack
  - Data-plane boundary: dashboard query results MUST be filtered by underlying resource visibility; dashboard access must not bypass event/execution/artifact/rule visibility checks

# Runtimes (Suggested Implementation)
- Runtime definitions should be visible based on runtime scope (system/global runtime vs pack runtime)
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) runtime read should grant access regardless of any other grants
  - System/global runtime metadata may be visible to authenticated users; pack-specific runtimes should require pack-scoped read
  - Runtime execution configuration details that include sensitive/internal values MUST be redacted unless caller has appropriate configure-level permission

# Audit Events (Suggested Implementation)
- Audit events are sensitive operational records and should not be broadly visible by default
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) audit read should grant access regardless of any other grants
  - Non-global access should require explicit audit-read scope (typically admin/security scopes)
  - Audit payload boundaries: secret values, token material, and protected payload fields MUST always be redacted regardless of caller visibility

# History (Suggested Implementation)
- History visibility should inherit the visibility of the parent entity (`execution`, `worker`, etc.)
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) history read should grant access regardless of any other grants
  - A caller may read history rows only when they can read the corresponding parent entity
  - History responses MUST NOT reveal previous values that would violate current non-leak rules for the parent resource

# Traces (Suggested Implementation)
- Trace reports are cross-resource aggregates and must be visibility-gated per linked entity
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) trace read should grant access regardless of any other grants
  - For non-global callers, trace output must be filtered to include only entities the caller can read
  - Trace-level summaries/aggregates MUST NOT leak counts, refs, or topology for inaccessible entities

# Workers (Suggested Implementation)
- Worker resources are operational infrastructure and should be admin/operations scoped
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) worker read should grant access regardless of any other grants
  - Non-global access should require explicit worker-read/manage permission; workers should not be visible to general authenticated users by default
  - Worker control operations (cordon/uncordon and related mutating actions) should require worker-manage permission and never be implied by read access

# Analytics (Suggested Implementation)
- Analytics endpoints are derived data and must preserve source-resource visibility guarantees
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) analytics read should grant access regardless of any other grants
  - Non-global analytics results should be computed only from rows/resources visible to the caller
  - Aggregate non-leak boundaries: analytics responses MUST NOT expose sensitive small-cohort information, private refs, or inferred private topology through grouped metrics

# Identities and Permission Sets (Suggested Implementation)
- Identity and permission-set resources are security-administration data and should be admin scoped
- Row-level results will be restricted in the following manner:
  - Un-scoped (Global) identity/permission read should grant access regardless of any other grants
  - Non-global access should require explicit IAM-administration scopes; these resources should not be visible to general authenticated users by default
  - Responses MUST redact sensitive identity security fields and internal grant evaluation details unless explicitly required by authorized admin workflows

# Implementation Gaps / Additional Considerations (Priority Order)

## Query Strategy and Performance (Global Requirement)
- Visibility filtering MUST be applied in-database, not as post-query in-memory filtering for final API result sets
- Per HTTP request, authorization-aware data retrieval SHOULD target:
  - one query in most cases
  - two queries when necessary (for example, resolving effective grants/context + final filtered query)
  - three queries only in rare, justified edge cases
  - cross-resource aggregate endpoints (traces, analytics, dashboards) are exempt from the one/two-query target and MAY issue one visibility-filtered subquery per distinct linked resource class
  - aggregate endpoint query counts MUST remain bounded by distinct resource classes (not row count), and filtering MUST still occur in-database (never via post-query in-memory filtering)
- Implementations MUST avoid N+1 authorization/database round trips for list/search endpoints
- Counts/pagination totals MUST be computed from the same visibility-filtered dataset returned to the caller (never from pre-filter supersets)
- Visibility logic SHOULD be pushed into repository-layer queries using authorization context derived once per request
- Parent/ancestor visibility and other hierarchical checks SHOULD rely on materialized/denormalized keys so the filtered query remains bounded (typically one query, occasionally two)

## MUST: Critical Visibility Corrections
- **History**: enforce parent-entity visibility inheritance (`execution`, `worker`, etc.); authenticated access alone is insufficient
- **Inquiries**: enforce participant/assignee/linked-execution visibility for read/list, not only respond operations
- **Workflows**: apply row-level visibility filtering to list/get/by-pack endpoints; do not expose workflow definition/linkage outside authorized scope
- **Triggers/Sensors**: enforce consistent scope checks across sensor list/get/lifecycle and trigger-linked sensor listings
- **Traces**: apply per-entity filtering inside trace report generation so inaccessible linked entities do not appear in aggregates or topology
- **Artifacts**: ensure list totals and pagination are visibility-aware in database and aligned with final rows

## SHOULD: High-Value Non-Leak Hardening
- **Actions/Packs/Queues**: replace broad fetch-then-filter patterns with visibility-constrained queries to prevent existence/count leakage
- **Dashboards/Analytics**: enforce aggregate non-leak controls (for example, no private topology leakage via grouped metrics and small cohorts)
- **Audit/Permissions/Policies**: keep strict admin-scoped visibility and apply consistent payload redaction for sensitive fields
- **Sensor Logs/Internal Files**: require inherited parent-resource visibility and avoid metadata side-channel leakage (artifact IDs, path existence, internal file structure)
- **Workers/Runtimes/Agent metadata**: classify infrastructure metadata as ops/admin scoped unless explicitly intended for general read

## Consistency Rules (Cross-Resource)
- List/search/get/download/stream endpoints for the same resource MUST use equivalent visibility semantics
- Expansion and aggregate endpoints MUST NOT broaden visibility compared with base row reads
- Derived visibility MUST fail closed: if a parent/linked resource used for visibility cannot be resolved (missing, deleted, or inaccessible), access is denied and MUST NOT fall back to a broader path
- For authoritative-path resources (for example, artifacts and rule-derived events/enforcements), if authoritative linkage/association is present but resolves to no readable target, the row MUST be denied and MUST NOT be reclassified as unlinked/unassociated for fallback evaluation
- For additive-path resources (for example, inquiries), an unreadable linked resource does not deny access when another first-class authorized path applies. Ownership/assignment/scope-reader paths are co-equal authorization paths, not fallback paths
- Alternate paths (for example, trigger-derived when no rule association, owner-derived when no execution linkage) apply only when no linkage/association record exists at all
- Error shaping SHOULD avoid revealing whether inaccessible resources exist (use consistent forbidden/not-found strategy per resource class)
- Error shaping for sensitive resources (keys, audit, identities/permissions) MUST avoid existence leakage and return redacted/sanitized responses by default
- Dynamic derived visibility (for example, rule-derived event/enforcement visibility) should be evaluated at read time and applied consistently in both row and aggregate paths
