# Candidate API effects and bootstrap admission

## Opt in to real candidate effects

Set `testing.candidate_api_effects: true` in `pack.yaml`. The worker must download
and verify the pinned candidate archive first. Tests of an already installed pack
cannot opt in without a candidate archive. Ordinary tests receive no credential.

The test runner receives two separate variables:

- `ATTUNE_CANDIDATE_EFFECTS_URL`, the exact API endpoint for this context.
- `ATTUNE_CANDIDATE_EFFECTS_TOKEN`, an opaque context credential.

POST JSON to that URL with the token in `X-Attune-Candidate-Effects`. This token
is not a JWT, integration-login credential, or archive-download credential. Do not
put it in `ATTUNE_API_TOKEN`. Production routes cannot consume it.

Supported operations are deliberately small:

```json
{"operation":"put_key","target":"example.test_key","value":{"message":"hello"}}
{"operation":"get_key","target":"example.test_key"}
{"operation":"put_trigger","target":"example.test_trigger"}
{"operation":"emit_event","target":"example.test_trigger","payload":{"message":"hello"}}
{"operation":"list_events"}
```

Targets must have the candidate pack's ref prefix. Creating a candidate trigger
does not resolve or change a production trigger with the same name. Emission
requires a trigger created in this exact context. Keys and events are actual
PostgreSQL writes through the candidate API service, not simulated responses.
Candidate triggers are isolated test resources, not deployed trigger contracts.
This endpoint does not provide production schema compilation, encryption,
signing, action execution, rules, queues, or notification delivery.

All effects live in `pack_candidate_resource` and `pack_candidate_event`.
They never enter production Key/Event tables, rule ingress, audit notifications,
or notifier fanout. IDs in candidate responses are not production resource IDs.
Responses use a `data` envelope.

The protected route uses `RequireAuth<AuthenticatedCandidateContext>`. Its
specialized guard authenticates the real live context before body extraction,
without creating human Claims or spending an operation. Effect admission repeats
all fences in its write transaction and counts the actual operation once. Default
`RequireAuth` remains JWT/user authentication on other protected routes. Neither
credential class can substitute for the other.

Each context allows at most 128 resources, 256 events, 1024 successful operations,
and 64 KiB per value/payload. HTTP requests are limited to 70 KiB. A runtime
credential expires after six hours and cannot be renewed or reminted for the
same install. The install's seven-hour deadline also applies.

Each effect locks the install before the context, then rechecks the assigned
worker, running status, exact archive key/provider version/digest, archive-token
hash, runtime-token hash, revocation and deadlines. Finish/failure/activation
updates take the install lock first and revoke/purge through a database trigger.
The worker also explicitly closes the context before persisting runner results.
An effect admitted before close may finish before close; after close commits,
the credential cannot admit another effect. A periodic API maintenance pass
purges abandoned expired effects. Revoked tombstones remain until their install
is deleted so cleanup cannot permit reminting.

For an opt-in test attempt, deployment activation must call
`CandidateEffectRepository::require_closed_context` with the install ID and the
candidate archive key, provider version and digest in the ordinary activation
transaction. It requires the `activating` state and an exact closed-context
binding. A successful result from an older worker without that evidence must
not qualify. Explicitly skipped tests do not call this hook. The service's
isolation checks do not replace this deployment-side evidence check.

This isolation covers Attune effects only. Pack code's external network or
filesystem effects are not rolled back. Use test-owned external dependencies.

## Bootstrap through the API

API/object and shared-volume jobs now use `bootstrap_core_pack.py` and ordinary
API upload admission. The former Python SQL loader is an API-only wrapper. Its
database/schema/storage-path flags are rejected. `load-core-pack.sh` forwards
arguments to that wrapper. The API's legacy release-upgrade scan reports packs
requiring explicit API readmission; it no longer freezes and activates mutable
metadata automatically.

The shared-volume init job assembles a private temporary candidate and can add
the separately built timer binary. It does not copy source declarations over
the active pack projection before admission. API activation owns publication.
The retained Compose `pack-release-upgrade` dependency is a no-op after API
bootstrap, not a second deployment writer.

Readiness distinguishes authenticated publication from managed-runtime enablement:

- `/health/ready` checks the exact platform catalog. Its `pack_publication`
  descriptor advertises protocol/client version 1, revision
  `authenticated-pack-publication-1`, registrar authorization, and the
  `immutable_release` and `atomic_deployment` components. The core publisher
  requires this exact contract before uploading with the operator's existing
  credential. The upload route still checks registrar grants and performs atomic
  admission. This descriptor does not grant an authorization exemption.
- The separate `bootstrap_admission` descriptor reports the broader managed-pack
  contract. `BOOTSTRAP_ADMISSION_ENABLED` remains false and
  `bootstrap_admission_protocol` remains null until that rollout is accepted.
  Ordinary authenticated publication does not enable that gate or establish
  acceptance of incomplete runtime consumers.
- `/health/bootstrap` also requires a current, unrevoked core runtime-policy
  revision bound to the exact active core release. It returns release ID,
  version, digest and policy revision. It does not wait for worker hosts.
- `/health/content` requires that admission plus fresh action/sensor hosts.

The publish job checks admission but does not wait for hosts that may depend on
its completion. The separate `wait` command waits for application content.
Supply an existing deployment integration token as `ATTUNE_BOOTSTRAP_TOKEN`.
The publisher authenticates it through the API. It never opens a database,
creates a human/service identity, assigns `core.admin`, or changes grants.
Helm accepts a Secret name/key through `corePack.deploymentTokenSecret` and
`corePack.deploymentTokenKey`. Compose passes the token only to `init-packs`.
Provisioning that deployment credential is an operator prerequisite for fresh
install, not an implicit SQL bootstrap privilege. Runtime consumers do not depend
on its continued activity after admission.

Normal upload and bootstrap share `DeploymentAdmission`. Its outer graph lock
precedes cache/pack locks. Existing configuration layers are preserved before
mutation. After component loading and structural sensor assessment, the same
transaction activates the immutable release, captures A/H runtime/graph/input
and concrete-target authority, captures rule input authority, binds ORIGINAL
authored defaults and preserved overrides, and readmits D sensor metadata.
Any error rolls back all database changes. Publication is still owned by the
ordinary API path. The internal pending-admission loader is used by ingress only
after coordinated activation. It grants no runtime authority itself and must be
followed by canonical admission before commit.

When configured, the publisher sends `ATTUNE_CORE_PACK_DIGEST` as multipart
`expected_release_digest`. The API compares that pin with the immutable published
archive before any active-release or component mutation. A post-upload mismatch
check alone cannot protect the old active pointer.

Readiness validates the root policy's full fixed-approval ceiling as well as
retained publisher dependencies. Merely finding a current policy row is not
enough when an approval has expired. Runtime effects still apply their own
selection-specific concrete target and approval fences.

A failed upgrade must leave the prior active release and current policy pointer
unchanged. The activation planner must capture and activate runtime policy in
the same transaction as the release/projections. Freezing release bytes alone
is not admission evidence. A previously unadmitted same-version core release
requires explicit operator readmission, not silent startup authorization.

Roll out compatible API, worker, executor and bootstrap images together. The
protocol check fences an old API from a new bootstrap job; it cannot stop an
old writer that never reads readiness. Revoke old writer credentials and drain
incompatible services as described in [Platform catalog](../packs/platform-catalog.md).

### Core composition and startup acceptance

`scripts/test_core_bootstrap_composition.py` inventories the actual `packs/core`
archive, including entrypoint bytes and executable bits. Bundled reserved roles
must match the platform catalog and stay platform-owned. `core.key_creator` is a
separate pack policy, not another reserved role. The shipped tree contains eleven
actions, including the canonical `core.install_packs` workflow action, seven
triggers, one timer sensor and nine runtime definitions. It intentionally ships
no queue declarations. Archive inventory alone does not prove admission.

`bootstrap_deployment_transaction_tests` copies that tree through an owned archive
and uses the ordinary deployment admission service with real immutable release
storage. It covers defaults, operator overrides, failed final admission, successful
upgrade and explicit readmission of retained code for rollback. Workflow graph
bindings and retained executable metadata come from the shipped installer, not
a supplementary workflow. The supplementary queue dispatches that real workflow
and is explicitly test-only content. Retained execution pins, platform-role bodies
and fixed approvals must survive installer or approval-issuer freeze independently.
The separate expiry case uses elapsed time rather than rewriting approval history.
These repository tests do not exercise protected upload authorization or dispatch
the timer launcher. Actual execution requires the packaged native timer binary.

Helm publication is a post-install hook. With `helm --wait`, ordinary workloads
must become ready before that hook runs. API, worker and web startup therefore
must not wait for `/health/content`, `/health/bootstrap` or the content-ready hook.
Publication waits for platform readiness; the later content hook checks admitted
pins and host availability. Compose can gate web on its separate content-ready
service, but neither publication nor any host prerequisite may depend on that
service. Offline render and dependency tests detect this startup cycle; they do
not prove Kubernetes hook execution, image compatibility or a live UI lifecycle.

Central PostgreSQL 18 validation must compile and execute the repository cases.
Full API-only fresh install, failed upgrade, rollback, sensor/action dispatch and
Helm/Compose lifecycle acceptance still require compatible images and an
operator-provisioned integration credential. Keep readiness disabled until that
coordinated acceptance completes.
