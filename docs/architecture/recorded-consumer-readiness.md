# Recorded execution consumer readiness

Recorded execution is currently denied by default. `RecordedConsumerReadiness`
in `crates/common/src/execution_authority.rs` requires every consumer, including
candidate isolation. Its missing-consumer diagnostics describe prerequisites;
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
| Candidate isolation | worker `service.rs` candidate test handling, runtime preparation/verification, common sandbox | actual preparation and verification isolation, writer cleanup, and supported deployment-profile feasibility |

Production enqueue, dispatch, scheduling and retry paths currently call the
unready default. Worker construction also defaults to unready; its readiness
builder has no production caller. Replacing these calls requires evidence and a
coordinated startup contract. Missing recorded evidence must never select an
older delegated credential or ambient service identity.

Candidate isolation is not established by a successful final-action sandbox
launch. Preparation, verification and candidate executable paths must use the
boundary, with supported profile prerequisites and cleanup proved. Host-only
fixtures cannot certify Compose or persistent Kubernetes service profiles.

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

This remains work in progress under disabled readiness gates. Contract fixtures
cover slices of the binding, not coordinated consumer readiness. Protected
contributor origins beyond locally supplied schema/queue-item payload fields,
malformed-proof and concurrent lifecycle coverage, complete affected-consumer
validation and SQLx preparation remain restoration requirements. Existing live
contributor credential resolution is not a substitute for these proofs.

Positive restoration evidence requires real publication/admission through
terminal item acknowledgement, including mixed enqueue-only contributors,
configuration changes, revocation, protected output and existing ad hoc queues.
A successful repository fixture, enqueue response or build is insufficient.
