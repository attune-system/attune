# Required explicit finite-input broker acceptance

These external-service tests are mandatory acceptance for the receipt-lifetime
proposal. Both require owned PostgreSQL and RabbitMQ and are explicitly ignored
in ordinary Cargo inventories with main's approval. Default ignored counts,
unexecuted selections, missing environment, setup failures, or unrelated input
tests do not satisfy this inventory. They are not deferred managed-pack SOW cases.

Run from the reviewed/applied repository root, serially:

```bash
bash scripts/run-receipt-input-deadline-acceptance.sh
```

For a single diagnostic run, set `ATTUNE_RECEIPT_ACCEPTANCE_FOCUS_TARGET` to
`lib` or `bin` and `ATTUNE_RECEIPT_ACCEPTANCE_FOCUS_CASE` to one exact case suffix
from the inventory below, without the shared module prefix. Both must be set.
The runner rejects unknown selections before creating services and labels the
result as focused coverage, not full twelve-execution acceptance. Leave both
variables unset for the required complete run.

Prerequisites are Docker, Python 3, psql, sqlx, cargo and make. The script owns
disposable PostgreSQL 18 and RabbitMQ 4.3.6 containers on random loopback ports,
initializes the test database with `make db-test-setup`, creates the two owned
vhosts, and exports their exact URLs. No fixed host ports, shared developer
broker, shared vhost, inherited broker URL or ambient database is accepted by
this runner. It generates a fresh Attune test run ID per Cargo invocation, uses
four test threads and runs the two targets serially. Logs remain under the
reported `/tmp/opencode/receipt-deadline.*` directory. A teardown failure fails
the runner even if every test passed.

The mandatory inventory is each of the following exact identities under BOTH
`--lib` and `--bin attune-executor`:

1. `scheduler::native_failure_tests::receipt_deadline::original_finite_input_actual_completion_commits_and_publishes_before_expiry_control`
2. `scheduler::native_failure_tests::receipt_deadline::original_finite_input_expires_at_actual_completion_commit_and_publishes_nothing`
3. `scheduler::native_failure_tests::receipt_deadline::publication_wait::original_finite_later_input_two_publications_before_expiry_control`
4. `scheduler::native_failure_tests::receipt_deadline::publication_wait::original_finite_later_input_expires_during_first_application_handoff_wait`
5. `scheduler::native_failure_tests::receipt_deadline::publication_wait::publisher_recovery::original_finite_input_real_publisher_recovery_before_expiry_control`
6. `scheduler::native_failure_tests::receipt_deadline::publication_wait::publisher_recovery::original_finite_input_denies_new_attempt_after_real_publisher_recovery_wait`

This is twelve mandatory exact executions, six identities in each target. The
runner provisions the original commit and publication vhosts plus isolated
`receipt-recovery-control` and `receipt-recovery-expiry`. New tests use the same
explicit PostgreSQL/RabbitMQ ignore reason, never a default-scope passing claim.

For a main-owned isolated service lane using the same vhost/URL contract, the
equivalent exact command for each identity is:

```bash
ATTUNE_TEST_RUN_ID=<fresh-id> CARGO_BUILD_JOBS=1 RUST_MIN_STACK=67108864 cargo +1.99.0 test --locked --offline --all-features -p attune-executor --lib '<exact-identity>' -- --exact --ignored --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=<different-fresh-id> CARGO_BUILD_JOBS=1 RUST_MIN_STACK=67108864 cargo +1.99.0 test --locked --offline --all-features -p attune-executor --bin attune-executor '<exact-identity>' -- --exact --ignored --nocapture --test-threads=4
```

The owned broker base is `ATTUNE_TEST_CACHE_BROKER_URL`, without a vhost suffix.
It must have isolated `receipt-input-control` and `receipt-input-expiry` vhosts.
`CacheBrokerFixture::connect` fails when the URL is missing or its resources are
unavailable. These tests never return early for missing services. Select exact
identities, not all ignored executor tests with unprovisioned unrelated vhosts.

## What the tests establish if they execute successfully

The fixture authors a new case before release/policy capture. It retains the
existing prospective-result program and its actual nonempty consumer inputs,
but gives a separate consumer action one additional secret field, `/finite`,
without a schema default. The root graph authors its exact task-input Key selector
under independently issued finite external input consent. Only actual child input
evaluation consumes that consent. Root accepted input, the completed predecessor,
and outer transition/source/output plans have no such deadline. This matters:
putting the finite Key on a root publication stage would let an already-retained
outer transition fence reject without exercising the discarded child proofs.
Canonical task-input evaluation, the accepted-input sealer and first typed-delivery
materializer create the actual child's finite evidence. The original input-only
approval is absent from root/child selected SDK authority. Its ID, original
expiry, grants and digest remain unchanged.

Both tests invoke the actual completion-driven scheduler transaction owner.
A test-only, instance-local observer runs after the full production mutation
phase and the actual child's original seal/delivery preparation, before the
owner's final proof collector and COMMIT. It validates the already prepared
child using canonical read-only reopening, follows its real receipt graph to
the original approval, signals readiness, and waits on an owned oneshot barrier.
No global hook, proof injection, expiry edit, re-admission or fixture reseal is
used. The observer returns Ok after release, even when its held receipt has
expired, so only the real production collector can supply the tested rejection.

The negative waits on a bounded `clock_timestamp()` predicate for that exact
committed approval ID and unchanged deadline, then releases the barrier. It
requires the actual owner to return PermissionDenied, roll back child/admission/
receipt and workflow business writes, and leave the owned broker queue empty.
The other connection never reads uncommitted child evidence. The positive
releases immediately, requires actual commit and the matching broker envelope,
then verifies original acceptance and no duplicate publication. Both tests await
the owner and all resource teardown before asserting scenario failures.

These are proposal tests, not recorded passes. Their compile/runtime result is
unknown until main runs the exact inventory on the reviewed applied source.
The additional publication_wait cases author two successors before capture:
ordinary first consumer, then a separate finite-input consumer. Production's
actual collector validates both fresh and commits before the instance-local
test observer holds the FIRST application's publication handoff. A separate
connection can see both committed rows before waiting on the original later
input approval's unchanged expiry. The observer returns Ok, first-child original
proofs stay valid and the real owner publishes that first request; production's
held per-child proof check must reject the expired later NEW local attempt.
The negative queue contains only the valid first request. Both child rows stay
committed; no rollback or withdrawal is claimed after commit. The before-expiry
positive control requires both messages from the same authored finite fixture.
Original approval identity, grants, digest and deadline remain unchanged.

This is an application-handoff wait, not a blocked broker Ack or channel lock.
The recovery cases additionally create a Publisher through a single-connection,
owned loopback AMQP passthrough before finite consent is issued. After actual
producer commit, both original requests are drained. Source-only reopening
retains the same actual finite Requested child's opaque proofs through a short
validation commit and the guarded publication future. The test closes only
that Publisher's actual channel, then holds the broker's NEXT Channel.OpenOk
at a bounded frame gate. This is a real recoverable first closed-channel error
and actual reset_channel/create_channel RPC wait, not an injected mock error.
After the unchanged original approval expires, releasing OpenOk must trigger
the real proof callback's Guard/PermissionDenied before a new publish attempt.
Transport timeout/error is not accepted as proof of expiry. The negative owned
queue stays empty; the before-expiry control receives exactly one request for
the same original row. No child, input, approval or ciphertext is replaced.

The optional public date budget is deliberately None ONLY in this recovery
probe, with a 150s owned-test transport timeout, to isolate the real original
120s proof callback after the wait from the public-date timer expiring first.
Authority is still the actual held original proof, never that optional date;
production callers' tighter date budgets are unchanged. No grace period is
introduced. Atomic enqueue, wire timing, withdrawal and socket deadlines are
neither provided nor claimed. Main must wrap the runner with durable memory,
deadlines, source integrity and ownership fallback before recording acceptance.

For supervised invocation, optionally set `ATTUNE_RECEIPT_ACCEPTANCE_RUN_ID`
to 1–20 lowercase letters/digits with optional non-leading hyphens, and set
`ATTUNE_RECEIPT_ACCEPTANCE_OUT` to a NEW directory beneath the wrapper's suite.
The script refuses existing output paths and creates a private mode-700
directory. Defaults remain a generated run ID and private mktemp directory.
`run-controls.json` archives safe full controls, expected names and labels.
No generated password is archived in setup/test logs.

Before invoking, main's existing supervisor must seed `resources.json` with
`owner` equal to that run ID and `containers` containing exactly
`attune-receipt-pg-<run>` and `attune-receipt-mq-<run>`. Both Docker creates also
carry `attune.validation.owner=<run>` alongside `attune.receipt-deadline=<run>`.
Fallback cleanup must verify the owner label before removing either expected
name, including create-response-loss cases. Direct script cleanup remains
limited to successfully returned create IDs. The wrapper owns source hashes,
sanitized Cargo environment, final cleanup verification and results JSON.
Pass a new subdirectory such as `SUITE/receipt`, not the existing suite root,
so its recursive log-progress observer sees the twelve serial invocations.

## Finite-input authoring contract

Secret parameter schemas retain `secret: true` and have no defaults. Canonical
schema validation forbids defaults at secret paths, including expression strings.
Changing a selector's spelling cannot make a secret schema default valid.

The single-child commit fixture authors the Key selector only at
`/workflow/tasks/1/input/finite`. The two-child publication fixtures author it only
at `/workflow/tasks/2/input/finite`. Both use
`{{ keystore['system.receipt_lifetime_original'] }}` so the complete Key ref is one
member selection. Returned Key strings are values and are never parsed as templates.

The approval owner is the root graph declaration under its original pinned
invocation and policy. The repository validates its existing declaration revision,
exact path and expression before issuing input-only consent once. The single-child
commit cases retain their original 20-second window; the pair and recovery cases
use the user-approved original 120-second fixture window described below.
It is not a callee schema-default approval, selected root SDK grant, root parameter,
workflow variable or transition control. Only actual child input evaluation reads
the Key and records its finite Value proof at the child's `/finite` destination.
No approval is renewed, expiry changed, historical child resealed or current IAM
consulted when retained proofs reopen for commit or a fresh publication attempt.

This correction does not count as runtime acceptance. Historical single-child
passes are evidence only for their recorded source state, not a current-state
certification. Gate first on the exact pair-positive lib case, then the pair
negative and recovery controls separately, followed by all twelve mandatory
lib/bin executions on the final reviewed state. A focused execution is never
full twelve-case acceptance.


## Commit fixture readiness budget

The commit-boundary fixture's former ten-second readiness cutoff was a harness
guard introduced during implementation, not a user security requirement or a
production latency contract. The actual authority bound remains the independently
issued original twenty-second INPUT approval and its unchanged receipt graph.

After the original consent commits, the test reads only that deadline's remaining
milliseconds on its already reserved clock connection. It floors the server
remainder, anchors the local timeout before the query, and caps both measurement
and readiness at an operator limit anchored before the original deadline query.
Query RTT, channel setup, spawning and scheduling consume the same original
window. A nonpositive/exhausted remainder fails; observing readiness after the
conservative deadline also fails. No new twenty-second wait starts at readiness,
and no local deadline grants input authority.

Both original commit cases use this shared readiness budget. Their original
twenty-second approval identity/expiry, real expiry-negative wait, source-only
proof checks, savepoint observer, commit collector and guarded publication remain.
If commit or publication cannot finish while the real original proofs are fresh,
the positive control still fails. This does not establish a throughput fix or a
passing broker treatment. All twelve exact executions remain mandatory.

The single-child treatment above does not define pair/recovery fixture timing.
The separately user-approved pair/recovery windows and owned transport bounds are
listed below. No production transport or authority defaults are changed.


## Pair and recovery fixture windows

The real pair-positive diagnostic run `a3a7c4d6` reached second-child sealing and
rejected its original retained approval at the existing activity-expiry check.
The guard was correct. The user explicitly approved 120 seconds for the pair and
recovery fixtures after that result. This is test setup for the current validation
profile, not a production default TTL, a latency target or a performance fix.
The passing single-child fixtures and their thirty-second expiry-poll wrapper
remain unchanged at an original twenty-second consent window.

The shared pair/recovery helper issues one original 120-second approval. It never
renews it at child initialization, commit, publication, recovery or a new attempt.
Approval ID, grants, declaration digest and original deadline are retained and
compared against the unchanged original snapshot. The actual production guards
still reject any stale proof. Negative cases wait for that exact approval ID and
deadline to expire on the database clock, so each may take about two minutes.

Pair first-handoff and recovery OpenOk readiness use the parent's existing
conservative arithmetic helper. On an already reserved clock connection they
read the original deadline's floored remaining milliseconds, anchor before that
query, and cap the result at an operator Instant captured before the original
120-second deadline query. Query RTT and scheduling consume the original window.
Late ready values are rejected after receipt. No new 120-second readiness window
starts at spawn or at an AMQP recovery gate.

Negative expiry polling and held first-handoff/OpenOk release use an absolute cap
at that same original operator limit plus ten seconds. This margin is only for
polling and releasing owned test resources, never positive readiness or authority.
The first nonfinite child remains authorized while the later child genuinely
expires. The observer does not inject an expiry error; production's separately
retained proofs decide whether a new local attempt is allowed after the wait.

Only the owned recovery Publisher uses a 150-second transport timeout. Its public
wait-deadline argument remains deliberately absent to isolate the original-proof
callback. Neither that timeout nor the ten-second control margin grants a stale
proof any grace. Connect, initial handshake, frame size, task ownership and
post-release/teardown bounds are unchanged.

The loopback proxy holds the inbound OpenOk frame and therefore delays following
server heartbeats too. Its client requests 180 seconds, and this runner's own
disposable RabbitMQ 4.3.6 container advertises 180 seconds using
`RABBITMQ_SERVER_ADDITIONAL_ERL_ARGS=-rabbit heartbeat 180`. The runner verifies
that server application setting before testing. Merely increasing the client
request while leaving a 60-second server proposal can still negotiate 60 seconds.
Heartbeats remain enabled. No ambient broker, Compose file, production MQ config
or other runner is changed. This is synthetic-gate transport setup, not heartbeat
reliability coverage. The bounded proxy still owns one connection/task, a 128-KiB
frame allocation and awaited shutdown.

All temporary pair diagnostics, their local subscriber and the equivalent
activity boolean temporary are removed in this source proposal. Useful actual
owner-error retention after awaited cleanup remains. Strict accepted-input
equality, full VALUE lineage, overwritten CONTROL, authority and physical-session
custody, Independent nonselection and no secret defaults remain unchanged.
All twelve exact lib/bin identities and the six deferred deployment cases retain
their status. No clean acceptance is claimed until the final full lane executes.
