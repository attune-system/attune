# Supported deployment test scope

The supported release keeps registrar exemptions, bootstrap admission and
managed credential gates off. Publishing a runtime policy still requires exact
initial registrar consent. Captured runtime authority must remain independent
of later human grant removal or freeze.

Workflow calls may remain self-recursive or mutually recursive. The user chose
to preserve that existing behavior. Task-transition loops and event/rule feedback
are unchanged. Deployment acceptance does not prove termination or require
executing an unbounded recursive workflow.

## Visible deferred coverage

Six tests in `crates/api/tests/self_contained_deployment_tests.rs` are an explicit,
narrow exception to the usual external-service/stress-only ignore policy.
Five retain paused self-contained-pack SOW scenarios in #112 that require
disabled registrar exemptions. The sixth preserves a historical recursion-negative
acceptance case whose overbroad compile-time-cycle expectation conflicts with
the chosen recursive-call policy. That historical case is not a future mandatory
recursion ban and may be revisited only with explicit graph-contract redesign.
All six original bodies remain unchanged, with reasoned `#[ignore]` annotations.
They still compile in normal workspace/all-target/all-features checks. Default
execution reports six ignored tests, not six successes. A release report must label them **deferred** and
must not claim full SOW deployment acceptance from a passing supported suite.

This keeps one dependency graph and lets normal libtest output expose the scope
gap. A feature gate would reinclude cases under `--all-features`, or hide them
behind configuration. A detached crate would add another dependency/lockfile
boundary and duplicate test infrastructure. Neither is needed here.

Only these six identities have the scope exception. It does not authorize
ignoring other failures. Revisit the five admission scenarios only after separately
authorized admission work and actual acceptance validation. Revisit the historical
recursion negative only after an explicit graph-contract decision. Invoking `--ignored`
or `--include-ignored` opts back into these known deferred cases; external-mode
Docker runs using `ATTUNE_RUST_INCLUDE_EXTERNAL=1` also opt into them. Supplying
external services alone does not make the deferred admission contract available.
The Docker artifact extractor's `inventory.tsv` contains only non-ignored tests.
Report its supported fingerprint alongside this six-case deferred inventory;
that fingerprint is not evidence that all declared SOW tests passed.

## Inventory of the original eleven tests

All names below are in the API `self_contained_deployment_tests` target unless
another target is stated. No expected rejection was changed to an arbitrary 4xx.

| Original identity | Supported or deferred | Coverage and reason |
| --- | --- | --- |
| `fresh_archive_uses_canonical_workflow_and_queue_authoring` | Supported, unchanged | Pure archive authoring grammar in both orders. |
| `missing_workflow_ref_fails_atomically_with_authored_location` | Supported, exact initial consent | Missing graph target and authored diagnostic, with no projected rows. |
| `recursive_nested_workflow_fails_atomically_with_authored_location` | Deferred historical graph-contract case, original body unchanged | User policy preserves recursive workflow calls. The original broad recursion rejection is not a supported release requirement or a future mandatory ban. A new supported positive checks real recursive upload and pinned call edges. |
| `invalid_workflow_transition_fails_atomically_with_authored_location` | Supported, exact initial consent | Invalid transition and authored diagnostic, with no projected rows. |
| `private_foreign_action_is_a_visibility_error_not_missing_external_approval` | Supported, exact initial consent | Each pack's own tick only; foreign task selects no runtime refs. Private visibility still denies; subject rows roll back and publisher remains active. |
| `existing_foreign_owned_ref_is_not_adopted_by_matching_pack_prefix` | Replaced at valid boundaries | Ordinary repository creation already rejects its impossible fixture. New API identity `repository_rejects_action_ref_outside_its_structural_pack` checks that error and absence of rows. New common loader unit test `pack_registry::loader::tests::matching_pack_prefix_never_substitutes_for_existing_structural_owner` checks matching-prefix acceptance cannot override a foreign or absent actual owner. No malformed row or raw service SQL is used. This is not an end-to-end corrupted-row upload acceptance claim. |
| `fresh_scoped_upload_pins_every_source_and_defaults_survive_installer_freeze` | Deferred, original scenario unchanged | Packs-only fresh deployment without initial Events/Create requires the disabled exemption. |
| `reversed_fresh_upload_resolves_nested_and_finite_targets_without_preallocated_ids` | Deferred, original scenario unchanged | Same no-initial-consent contract in reversed fresh declaration/archive order. |
| `unused_global_policy_definition_cannot_bypass_external_approval` | Deferred, original scenario unchanged | Approval-specific diagnostic is masked by the supported registrar denial on the local definition. Still requires future approval coverage even for unused definitions. |
| `unapproved_external_scope_fails_atomically_with_authored_location` | Deferred, original scenario unchanged | Its Packs-only publisher cannot install, and the external subject needs the future approval-aware registrar contract. |
| `fixed_external_approval_is_reused_without_deployer_runtime_grants_after_issuer_freeze` | Deferred, original scenario unchanged | Independently approved external scope without deployer runtime grants needs the disabled registrar exemption. Granting external Events/Create to the deployer would invalidate the scenario. |

## Added supported coverage

* `packs_only_installer_cannot_publish_tick_runtime_policy` expects the exact
  located registrar denial and no rows. Pack install never becomes policy consent.
* `exact_consent_upload_captures_defaults_before_consent_removal_and_installer_freeze`
  and `reversed_exact_consent_upload_captures_defaults_before_consent_removal_and_installer_freeze`
  install with only Events/Create on `consented_upload.tick` in addition to exact
  Packs rights. They check ten pinned invocations and captured defaults, remove
  the fixture actor's runtime consent, verify direct event creation is forbidden,
  freeze the installer, then create action/workflow executions through a distinct
  execute-only invoker using the captured defaults. These are admission and
  execution-creation checks, not worker dispatch or event-emission E2E evidence.
  The active pack read primes the grant cache before freeze; both warm-cache
  and post-invalidation reads must return 403 afterward. Access-token grant
  loading freshly checks the human identity, without changing recorded runtime
  or sensor authority. This is not a claim that every JWT-only endpoint revokes
  an otherwise cryptographically valid token.
* `recursive_workflow_calls_upload_with_exact_consent_and_pinned_edges` uses
  exact initial tick consent and ordinary authenticated multipart upload. The
  outer workflow calls inner, and inner calls outer. It requires 201, an active
  release matching the captured policy, and immutable callee snapshots for both
  authored edges. It does not execute the recursive workflow.
* `repository_rejects_action_ref_outside_its_structural_pack` and the common loader
  ownership unit test preserve creation and structural-owner negative coverage.

The target contains fifteen tests: nine supported and six
deferred. The added common ownership unit test is separate. Actual pass counts
must come from a completed run; this scope document is not validation evidence.

Existing `pack_registry_tests` retains its pre-mutation self-broadening negative,
restricted publisher/default negatives and held-grants positive. Existing
`runtime_authority_repository_tests` retains fixed approval, concrete-target,
revocation and human-freeze coverage. None is ignored by this exception.

## Rerun commands

Run from the repository root after the source freeze ends, using independently
owned disposable PostgreSQL and the normal documented test fixture environment.
Keep connection credentials in the environment. Use a fresh run ID per invocation.
These commands enable no production gate.

Supported deployment target, including every supported original identity above:

```bash
ATTUNE_TEST_RUN_ID=deploy-supported-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests -- --nocapture --test-threads=4
```

For any one supported row, append its full function name before `--` and add
`--exact` after `--`. The expected complete default summary is nine supported
results and six visibly ignored deferred cases, not fifteen passing tests.

All six deferred scenarios remain deliberately runnable with their unchanged
assertions. The five admission scenarios are expected to remain red while their
contract is disabled. The historical recursion negative is expected to remain
red under the preserved recursive-call policy:

```bash
ATTUNE_TEST_RUN_ID=deploy-deferred-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests -- --ignored --nocapture --test-threads=4
```

For a particular deferred row, put that table's full function name before `--`
and use `--ignored --exact --nocapture --test-threads=4` after `--`. This selects
the original scenario, never a supported-consent replacement.

Exact per-scenario deferred reruns:

```bash
ATTUNE_TEST_RUN_ID=dep-sow-unused-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests unused_global_policy_definition_cannot_bypass_external_approval -- --ignored --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=dep-sow-fresh-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests fresh_scoped_upload_pins_every_source_and_defaults_survive_installer_freeze -- --ignored --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=dep-sow-reverse-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests reversed_fresh_upload_resolves_nested_and_finite_targets_without_preallocated_ids -- --ignored --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=dep-sow-denied-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests unapproved_external_scope_fails_atomically_with_authored_location -- --ignored --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=dep-sow-fixed-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests fixed_external_approval_is_reused_without_deployer_runtime_grants_after_issuer_freeze -- --ignored --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=dep-hist-rec-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests recursive_nested_workflow_fails_atomically_with_authored_location -- --ignored --exact --nocapture --test-threads=4
```

Preserved recursive-call deployment positive, without executing recursion:

```bash
ATTUNE_TEST_RUN_ID=dep-rec-positive-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --all-features --test self_contained_deployment_tests recursive_workflow_calls_upload_with_exact_consent_and_pinned_edges -- --exact --nocapture --test-threads=4
```

Structural-owner boundary and existing authorization regressions:

```bash
ATTUNE_TEST_RUN_ID=deploy-owner-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-common --lib pack_registry::loader::tests::matching_pack_prefix_never_substitutes_for_existing_structural_owner -- --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=deploy-registrar-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --test pack_registry_tests restricted_uploader -- --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=deploy-pregrant-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-api --test pack_registry_tests assigned_pack_set_cannot_be_broadened_to_authorize_its_own_replacement -- --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=deploy-approval-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-common --test runtime_authority_repository_tests deployment_reuses_external_approval_and_only_auto_approves_actual_installer_coverage -- --exact --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=deploy-freeze-01 timeout --signal=TERM --kill-after=60s 40m cargo test -p attune-common --test runtime_authority_repository_tests admitted_local_and_external_effects_survive_human_freeze -- --exact --nocapture --test-threads=4
cargo fmt --all -- --check
cargo check --all-targets --workspace --all-features
```

A timeout, failed cleanup, build failure or source hash mismatch is inconclusive,
not a pass. Complete applicable CI validation on the final exact state separately.
