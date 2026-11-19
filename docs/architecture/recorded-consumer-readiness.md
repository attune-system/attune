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

Current recorded dispatch instead resolves pack configuration and referenced
Keys against all item contributors. That path cannot implement the first proof
and rejects legitimate enqueue-only contributors. Accepted input receipts and
worker origin validation also consume contributor evidence, so changing only the
dispatch resolver is insufficient. Queue-owned configuration delivery needs its
own exact binding through acceptance, receipt sealing, worker restoration and
retry, without broadening contributor or runtime API grants.

Positive restoration evidence requires real publication/admission through
terminal item acknowledgement, including mixed enqueue-only contributors,
configuration changes, revocation, protected output and existing ad hoc queues.
A successful repository fixture, enqueue response or build is insufficient.
