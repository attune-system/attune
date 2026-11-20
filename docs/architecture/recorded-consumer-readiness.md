# Recorded execution consumer readiness

Recorded execution is currently denied by default. `RecordedConsumerReadiness`
in `crates/common/src/execution_authority.rs` requires every authority and secret
delivery consumer. Its missing-consumer diagnostics describe prerequisites;
they are not a deployment toggle or evidence that a consumer is ready.

## Consumer inventory

These are implementation and test entrypoints, not a readiness certification.
Tests using directly admitted fixtures exercise their boundary contracts without
establishing coordinated production startup or successful queue execution.

| Prerequisite | Implementation entrypoints | Boundary evidence to run |
| --- | --- | --- |
| Execution admission | `repositories/execution_runtime_admission.rs`, `repositories/runtime_authority.rs` in common | common `execution_admission_tests`, `runtime_authority_repository_tests` |
| API consumers | API `auth/middleware.rs`, `recorded_consumers.rs`, `routes/*/recorded.rs` | API `recorded_execution_events_tests` and its `recorded_consumers` modules; reject unconverted routes |
| Artifact disclosure | API `recorded_disclosure.rs`, `recorded_files.rs`; common origin repositories | API recorded file/artifact disclosure and protected completion cases |
| Runtime read provenance | API Key routes; common `repositories/runtime_read.rs`, `key_access.rs`; worker `secrets.rs` | recorded Key classification and worker `recorded_input_restoration_tests` |
| Notifier delivery | notifier `recorded_authority.rs`, `websocket_server.rs` | notifier `recorded_authority_transaction_tests` including revocation/delivery fencing |
| Worker completion | worker `executor.rs`, common staged completion and worker-delivery repositories | worker `support/recorded_completion_cases.rs`, recorded restoration tests |
| Native executor consumers | executor scheduler, retry manager, workflow/native common repositories | common native materialization, retry and outcome cases plus executor consumer tests |

Production enqueue, dispatch, scheduling and retry paths currently call the
unready default. Worker construction also defaults to unready; its readiness
builder has no production caller. Replacing these calls requires evidence and a
coordinated startup contract. Missing recorded evidence must never select an
older delegated credential or ambient service identity.

### Current startup and compatibility gaps

- `WorkerService::new` constructs `ActionExecutor` with its default readiness.
  `ActionExecutor::with_recorded_readiness` validates the complete contract but
  has no production caller. Worker registration advertises
  `delegated_execution_authority_v1` when encryption is configured; that is not
  recorded execution, input restoration or completion support.
- Scheduler worker selection filters delegated capability for delegated work.
  It has no corresponding recorded-worker compatibility filter. A registered
  worker or successful heartbeat therefore cannot establish recorded readiness.
- API authentication validates recorded credentials and rejects routes without
  an explicit `RecordedRuntimeConsumer` marker. This protects unconverted
  routes, but a marker alone does not prove the route's data-origin, revocation
  or commit/delivery fences. API health exposes catalog compatibility rather
  than a coordinated recorded-consumer handshake.
- Notifier source and notification checks retain independent authorization and
  delivery fences. Their presence does not establish API, worker or executor
  support, and there is no shared startup handshake between these services.
- Queue admission, dispatch, scheduling and retry still deny through default
  readiness. The API also retains its explicit recorded enqueue rejection.

The [coordinated authority cutover](../deployment/postgresql-only.md#execution-authority-cutover)
requires compatible service releases before admission resumes. Any production
wiring must preserve that release contract, reject unsupported workers, and
validate the actual owning consumer paths. Neither configuration booleans nor
process-hygiene and executable-sink inventories certify these seven consumers.

The current catalog cannot identify an expected service rollout.
`platform_catalog_state` stores only `singleton`, `compatibility_epoch` and
`revision`; pack-release and publication descriptors identify pack content, not
API/notifier/executor replicas or their expected boots/generation. Worker rows
cover Action/Sensor hosts only. The operator-owned PostgreSQL foundation below records expected membership;
production aggregation still has no deployed membership writer or service integration.

The pure `execution_authority::consumer_contract` protocol models role-local
descriptors and validates them against an explicit expected membership. It has
no production caller, transport, advertisement or readiness conversion. The
operator repository below is a foundation, not service registration or readiness. Its checked snapshot is not authority evidence. One health response,
an empty expected set, or support claimed by another role cannot satisfy it.

Actions and sensors are trusted, inspected, approved cooperative workloads.
Hostile sibling-process containment and candidate process isolation are not
prerequisites for recorded authority or queue operation. Environment clearing,
descriptor control, opt-in tokens, purpose-scoped credentials and secret-safe
stdin delivery remain required. See the
[trusted-workload plan](../plans/local-process-workload-isolation.md).

## Managed queue authority boundary

Existing `InvocationPolicy::dispatch_template_digest` and
`managed_dispatch_metadata_revision` bind immutable queue mapping/target
metadata. `work_queue_item_runtime_admission` records selected runtime authority
and requester attribution. Neither record is a queue-owned credential input
grant. Input materialization and optional execution API access remain separate.

Restoration must preserve three distinct proofs:

1. The exact accepted queue template, pinned target/schema, configuration
   revision and referenced credential origins were admitted by an authorized
   active owner. Later projection/configuration edits cannot rebind accepted
   items to different credentials or mappings.
2. Each record contributor authorizes their supplied data and its origins.
   Fixed queue credentials do not require enqueue-only contributors to possess
   plaintext Key rights. Contributors cannot choose arbitrary Keys by changing
   payload fields, and their grants must never be unioned.
3. Runtime API access uses the explicitly selected execution admission. Receiving
   a credential parameter does not grant Key API access. Protected completion,
   acknowledgements, retries and leases retain their existing attempt fences.

The non-activation implementation keeps fixed input material in the canonical
v1 `work_queue_item_configuration_input` owning record. Its stable ID and exact
item/queue/policy/configuration, target/schema, declaration and owner-qualified
Key bindings are also authenticated inside its encrypted envelope. Key value or
configuration drift does not select new bytes for historical items. Applicable
policy and runtime-owner revocation still denies new effects. Record-dependent
Key selectors are rejected deterministically at admission.

Managed dispatch has an explicit Queue branch in the existing canonical v1
`runtime_accepted_batch_manifest` / `runtime_accepted_batch_item` records. Each
member retains its own fixed-input reference, independently captured contributor
manifests, original payload/schema and encrypted protected payload fields.
Canonical validation requires exact membership, execution attachment, unique
dense member ordinals and a compatible admitted configuration cohort. The Action
branch retains its separate invariants. Typed render selections preserve each
member's provenance; a fixed-input receipt never supplies contributor or runtime
API grants. Queue-authored sites require their exact fixed-input ID, including
record-only expressions; missing bindings cannot fall back to nonqueue reads.

Retries copy the accepted manifest and ciphertext. Original item IDs remain
retained source references rather than lifetime owners. Owning fixed inputs can
be reclaimed only after their original item, receipt references, accepted batch
references and file-evidence pins are gone, under exclusive graph admission.
Configuration and policy evidence remains retained independently of active
authority. File-evidence closure and offline encryption authentication include
the owning inputs and accepted manifests.

Contract fixtures cover mixed fixed/record expressions, structured secret
subleaves, independent direct-Key ceilings and internal admitted-component
receipt transfers. Transfer issuance validates the actual source attempt,
selected authority, delivered field and exact destination queue/item. Retained
consumption permits completed or removed source executions while independently
checking retained policy, origin and disclosure fences. Authenticated per-field
transfers determine the exact retained parent-receipt ID set; those IDs alone
never authorize delivery. Corruption, conversion rollback and last-reference
reclamation cases exercise the owning record and canonical graph.

This remains work in progress under disabled readiness gates. These fixtures do
not establish coordinated consumer readiness. The public enqueue API still
accepts literal secret fields only; its internal typed-transfer seam is not
user-facing protected contributor support. The owned PostgreSQL capture/reclaim
case also checks shared/exclusive graph ordering, cancellation with a bounded
statement deadline, and visibility of committed source references. Complete
affected-consumer validation and SQLx preparation remain restoration
requirements. Existing live contributor credential resolution is not a
substitute for these proofs.

Positive restoration evidence requires real publication/admission through
terminal item acknowledgement, including mixed enqueue-only contributors,
configuration changes, revocation, protected output and existing ad hoc queues.
A successful repository fixture, enqueue response or build is insufficient.

## Operator-owned membership foundation (not activated)

Forward migration `20261119000007` adds append-only staged deployment generations
and a separate singleton active pointer. This state is independent of platform
catalog state, pack releases, authority grants and retained receipts. Stage does
not activate. Members may repeat a service role for replicas, but instance and
boot UUIDs must each be unique. Each member asserts its role's complete fixed
mask; overlapping masks remain conjunctive in the pure validator. Canonical
stored JSON and a database-generated SHA-256 digest retain the exact expected
set. The active pointer has a monotonically incremented revision and records the
activating session user, backend PID and database time. Activation locks that
pointer, compares its prior ID/revision, and revalidates the exact staged digest.

The boot model is **exact preassigned boots**. The offline deployment owner must
assign or collect every expected boot before staging/activation. Every process
restart/replacement requires a newly staged authoritative generation. Observed
services cannot approve new boots, change expected membership or elect themselves.
If this model is operationally unacceptable, redesign must precede integration;
observations cannot silently become the membership authority.

`RecordedRolloutRepository` supplies stage/load/active/activate seams only. The
mutation functions require exact direct `session_user` membership in the
separately DBA-created NOLOGIN role named by
`recorded_rollout_operator_role()`. Its installation-specific random name is
metadata, not a role grant. Migration creates
no roles or membership grants. The DBA must separately grant function EXECUTE
and read access to the offline deployment login; runtime logins receive no
mutation grants or operator membership. Inherited membership and `SET ROLE` do
not satisfy the direct-session check. SECURITY DEFINER functions use a fixed
migration-selected installation schema plus `pg_catalog`; temporary schemas are explicitly searched last. That schema must be
DBA-owned and unavailable for service CREATE. Migration rejects installation CREATE ACL grants beyond the schema owner.
Runtime logins must not be database
owners/superusers or have schema/trigger/function replacement privileges. A DBA
can always change database enforcement; this boundary does not constrain DBAs.

Writer inventory: SQLx migration setup owns schema installation; platform catalog
reconciliation owns catalog definitions; the offline encryption conversion tool
owns a distinct maintenance capability. None supplies a reviewed deployment
membership stage/activate command. The future offline operator/deployment caller,
its credential provisioning and deployment sequencing remain integration gaps.
There is no HTTP/MQ mutation route, service caller, observation transport,
scheduler/admission binding or readiness conversion in this foundation. An active
expected set alone supplies none of the seven consumer proofs. Retained work does
not require continued membership residency.
