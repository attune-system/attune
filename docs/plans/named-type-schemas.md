# Named type schemas

## Status

Implemented and validated. The common compiler, immutable runtime bindings,
registry-backed consumer writes, and shared rendering are implemented. Full
non-performance Tier 3 service validation passes all 66 cases on verified current
service binaries; frontend revalidation passes 389 tests and TypeScript checking.
This plan began with a static review
on 2026-10-12; the evidence below records later implementation and test results.

Latest acceptance evidence (2026-10-15):

- Full Tier 3: 66 passed; four performance-marked cases intentionally deselected.
  Evidence: `.scratch/named-type-validation-20261014/schema-volume-1e0074b29117-services.log`.
- Focused operational-metadata/sensor security: four passed, including historical
  protected-log reclamation and recorded-origin disclosure.
- Scheduler: 84 passed; six unrelated broker-only cache protocol cases were not
  provisioned in the direct database run. Full-stack cache lifecycle cases pass.
- Artifact repository: 40 passed, including sealed-object policy-override cleanup.
- All-target workspace compilation: passes without warnings.
- Frontend: final rerun passes 389 tests across 65 files and TypeScript checking.
- `git diff --check` passes. Final full-stack containers and Compose volumes are
  removed by their owning runner; private diagnostics and source files are retained.

No named-type SOW implementation or acceptance gaps remain. Performance-marked
E2E cases and the separate broker-only cache protocol suite are outside this
acceptance run, not silently counted as passing. Changes remain uncommitted.

The chronological notes below retain failed-run diagnoses and superseded pending
states; use this summary and the latest entries for current validation status.

The first common-compiler slice is implemented in
`crates/common/src/value_schema/`. Its explicit vocabulary, annotation/default
rules, registry-view obligations, and limits are documented in
[the compiler contract](named-type-schema-compiler.md). Existing API parameter
validation, inquiry options, queue schema definitions, and pack/archive schema
checking now use it. Type registration now includes file discovery, candidate
graph preflight, immutable release capture, and reverse-dependency protection.
Manual execution and enforcement creation now persist immutable action bindings,
validate/default/redact inputs under them, and inherit enforcement bindings at
execution handoff. Bound scheduling preserves persisted defaults, and workflow
context reconstruction validates restored input against its binding. Workflow,
queue-dispatch, and callback child creation now attach contracts transactionally.
Workers validate delivered input and use bound output contracts for warning-only
conformance, encryption, and mirroring. Static workflow task snapshots protect
their releases and prevent deletion of packs needed by admitted work.

Authenticated API, CLI, and MCP discovery and previews are implemented. Keys
support single-value schema assignment, retained bindings, explicit rebinding,
and validated encrypted writes. Queue items retain admission contracts through
updates and dispatch, with encrypted secret payload fields and recorded queue
origins. The shared schema editor preserves authored references and nested
constraints; Key forms support type search and preview comparison.

Inquiries now retain response bindings, protect secret options and responses,
allow opaque option selection, and require explicit decrypt plus visibility and
origin access for plaintext detail. Workflow delivery restores and validates
inquiry values and preserves their sensitivity in templates.

API event and webhook admission now bind emitted payloads to `out_schema` and
sensor-supplied configuration to the new flat `event_config_schema`. Webhook
metadata uses a server-owned contract with encrypted keys and headers. Event
contracts and encrypted fields are immutable; DEFAULT partition repair preserves
them, and bounded retention cleanup removes expired owners. Event-derived
execution values retain their original event identity for disclosure checks.
Historical event displays use the stored graph, not the current trigger.

Sensor admission now captures generation-scoped configuration and trigger
contracts, encrypts bound configuration, and retains released cache grants.
Sensor event admission uses the generation's stored contracts. Startup values
and initial rule configurations use one bounded stdin JSON object across the
manager, core timer, and Python/JavaScript/Java SDKs. Token issuance and cache
access no longer depend on the installer's current access.
Approved sensor JWT signing now uses explicit named
`config.signing_permission_set_refs`, release-captured Key `use` grants, and the
operator signing profile, without granting plaintext Key access.
Initial trigger-instance parameter values are defaulted, validated, and copied
at generation admission rather than refetched during process startup. Only
secret-marked resolved values in that copy are encrypted. Rules store only whole
Key templates for secret destinations, including nested fields. Schema compilation
prohibits secret defaults, including template-shaped defaults. API writes, pack registration, and runtime admission reject direct secret
values. Rule definitions do not store resolved credentials. Sensor templates use
deployment-checked `config.input_permission_set_refs`, capturing Key read/decrypt
and Pack read/decrypt grants separately from signing authority. Log versions now record
their producing generation; sensor-log and artifact content routes check its
retained contracts, decrypt authority, and original rule access. Unbound legacy
log files are no longer served as a plaintext fallback. Sensitive mirroring stays
disabled.
Sensor-derived event secrets retain the original generation input origins.
Event detail applies per-origin disclosure checks and audits only fields actually
revealed; entity-level event decrypt permission alone is insufficient.

Pack and sensor configuration now have immutable schema/value revisions and
encrypted secret fields, with redacted current projections. Configuration writes
apply defaults and validate complete values; deployment schema migration restores
preserved values and rejects incompatible changes in the activation transaction.
Explicit plaintext API reads require owning-resource decrypt authority. Derived
values record configuration revision origins. Platform alert and work-queue event
producers now use transactional event schema binding. Shared forms have bounded
resolved object/array controls, and inquiry/queue edits use stored contracts.
Key creation and complete-value replacement use the same single-value controls;
ordinary Key edits retain the stored graph. Pack forms retain extra fields and
omit unchanged configuration rather than submitting redacted placeholders.
Execution create/detail responses expose the stored input/output contracts without
static task executable metadata. Execution parameter and result displays use
those contracts instead of current action schemas. Rule rendering rejects events
without admission bindings.
Resolved manual-execution forms compare preview semantics with server acceptance
without sending revision selectors. Queue pack-config rendering checks the
attributed requester's read/decrypt rights and records the immutable configuration
revision instead of a mutable schema origin.

Pack-installed rules now use a system-owned pack runtime identity and separate
deployment-captured `input_permission_set_refs`. Captured input authority follows
enforcements, workflow children, and retries without granting action API access.
Mutable rule-input changes require the caller to cover those captured grants.
Workflow schema declarations from action metadata, workflow metadata, and graph
files must agree; graph-only declarations also bind at admission.
Normalized secret-origin edges protect referenced configuration revisions and
sensor generations during bounded supervisor reclamation. Log streams recheck
disclosure authority and token expiry before each frame. Process diagnostics use
generation-bound version paths and omit protected stderr from status and alerts.

Rule API authoring now resolves named schemas under the declaring component's
pack dependency and reference policy. Registration retains its candidate registry
for rule parameter validation. Both paths enforce whole Key templates for secret
destinations. Preserved secret configuration migrations require read/decrypt
authority when contract semantics change; equivalent deployments retain the
existing ciphertext and configuration revision without decrypting. Component
consumer edges now participate in type-removal and visibility-restriction checks,
and configuration binding replaces only its own reverse-reference edges.
Queue definition parsing has an explicit registry-backed entry point used by
registration. Direct repository writes compile named and inline definitions
transactionally before mutation and maintain reverse-reference edges. Focused regressions pass for
invalid defaults and prohibited secret literals. Rule authoring validates a
defaulted copy while keeping stored parameter maps unchanged; all 26 API
parameter-validation tests pass. API and registration boundary regressions for
required defaults now pass in pack registration. All 54 selected pack-registry
tests pass; the S3-only archive test remains ignored for its explicit storage-harness
prerequisite. All three API rule-update tests pass with create/update checks for
required defaults, preserved selector maps, and literal-secret rejection.
All 21 action-repository tests pass, including validation before create/update
mutation and awaited rollback cleanup. Workflow manual
execution preserves explicit empty values and reports preview/acceptance contract
changes without sending selectors. Process tests cover large stdin delivery with
full output pipes, fresh-process redelivery, EOF, and sanitized delivery timeouts.

Full-stack startup no longer needs Docker Desktop checkout sharing for this
validation run. Declared bind inputs were hash-verified and transferred into an
owned Docker volume. The first run exposed stale E2E fixture schemas and layout;
the next run reached timer and webhook scenarios but failed date-timer delivery
and webhook enforcement. Webhook enforcement incorrectly required unread
transport credentials during payload-only restoration; that path now validates
only its payload, while complete event reads still require metadata validation.
The four event-section tests, including the payload-only regression, pass.
The rebuilt webhook service regression passed. The isolated date-timer test also
passed on the same rebuilt images without a timer code change after resource
pressure eased. A subsequent rebuilt Tier 1 run passed all 31 tests. That snapshot
includes the initial SDK provenance changes, but predates the latest download
guards and aggregate provenance read checks, which passed separate API and
repository regressions. Managed-sensor service coverage remains outstanding.
The rebuilt Tier 3 cache-sensor authorization test passes with an explicit,
sensor-scoped `triggers:decrypt` fixture grant. Its first run correctly denied
log disclosure because the bundled administrator role lacks that grant.
Bundled administrator permissions and the disclosure policy are unchanged.
The broader rerun failed its cache-retention maintenance test and stopped before
the SDK Key-log probe. Supervisor was missing the run-owned RabbitMQ vhost
override and exited before maintenance. The Compose override is fixed, with an
isolation guard that failed before the fix and passes afterward. The lifecycle
service rerun passes, retaining the active generation and reclaiming expired
retired and abandoned generations after cursor expiry.
The full Tier 3 attempt passed seven tests before an HTTP read timeout in the
multiple-webhook-post scenario. It is not a passing tier run. The host had nearly
exhausted swap during concurrent compilation. An isolated rerun without
compilation reproduced the stall. Its SQL
waits on named-type graph admission for 60 seconds while the supervisor and
enforcement renderer stop progressing. The renderer acquires pack-configuration
graph admission through a second pooled connection while its outer transaction
already holds admission. A queued exclusive reclaimer can create a cross-connection
wait cycle. The deterministic queued-reclaimer regression reproduced the rendering
timeout and awaited teardown before failing. Rendering now reuses the enforcement
transaction for action, pack-configuration, event, and Key reads. The queued-
reclaimer regression passes after the fix. The rebuilt webhook service check
also passes: three webhook POSTs produced three events and six executions across
two rules, with one test passed and 68 deselected in 11.32 seconds. The owned
project `attune-e2e-schema-volume-a2d3f12b5f9a`, its Compose volumes, and source
volume were removed after diagnostics were saved. This focused result does not
certify the full Tier 3 suite. The first post-reboot build exposed a zero-byte
cached notifier executable; the service Dockerfile now discards empty cached
binaries before compilation and rejects empty outputs before image assembly.

Sensor SDK Key reads retain implicit read/decrypt access to pack-owned Keys, as
explicitly requested. Successful plaintext reads now record bounded immutable
Key ownership origins under the fenced generation. Secret sensor events copy
those origins, and log disclosure checks all recorded SDK reads. Pack-scoped
sensor logs are protected even when their schemas declare no secrets; stderr
summaries and service mirroring follow that conservative policy. These changes
have a forward migration applied to the owned test database. The API regression
passes for implicit SDK access after installer freeze, idempotent origin recording,
immutable and bounded metadata, stream revocation, Key read/decrypt disclosure,
and event provenance. Restart coverage verifies stale-token rejection, independent
new-generation origins, historical log disclosure, and generation-local cleanup.
All six Key authorization tests pass with these checks. Workspace compilation
and all three rule-update authorization tests pass, including rejection of
literal secret parameters and preservation of whole Key templates.
SQLx preparation passes for that slice. Plain sensor-log and artifact download
bodies now recheck disclosure after reading each chunk, and tail reads recheck
after fetching bytes. All six Key authorization tests pass with the plain sensor
and artifact download revocation regressions compiled and exercised. The workspace
all-targets recheck passes without warnings. All 23 sensor-workload repository tests pass, including SDK origin count,
aggregate-byte, duplicate-at-capacity, and corrupted-metadata read boundaries.
All 54 sensor library tests pass, including stdin EOF/redelivery, full output
pipes, sanitized delivery timeouts, environment isolation, and process reaping.
Integrated secret-bearing sensor lifecycle coverage for this authority is still required before removing
the sensitive-startup gate.

A test-only managed SDK probe now reads a synthetic pack-owned encrypted Key and
writes it to both captured log streams. Its Tier 3 regression passes and checks
implicit access without input grants, source read/decrypt requirements for log
disclosure, and audit redaction. It declares no secret inputs or event outputs,
so it exercises SDK-read authority without bypassing the startup gate. With both
service-log mirrors explicitly enabled, the synthetic value is absent from the
complete sensor service logs. The successful run includes fixture cleanup and
owned-stack teardown.

Mutable action, trigger, sensor, workflow, pack, and queue declarations now use
transactional registry-backed validation instead of the inline-consumer gate.
Registration supplies its candidate view; ordinary writes enforce declaring-pack
dependency and reference policy. Updates lock the owner after graph admission and
replace reverse-reference edges with the effective fields. Deletion removes a
consumer's edges atomically, preserving independently admitted release pins.
Workflow graph-only schemas participate in validation and edge tracking, including
the full-pack reference refresh. Inquiry
creation now reaches its existing creator-bound named resolution path without an
inline gate. All 92 action, sensor, trigger, queue, and consumer repository tests
pass. They cover failed-update rollback, unowned public-type admission,
private-type rejection, retirement protection, concurrent field updates, deletion,
and preservation of outer transaction rollback scopes. Cross-component registration
initially found an offline queue checker still selecting an empty registry. Its
syntax/metadata pass now feeds the shared local graph checker; deployment retains
full candidate-registry compilation. The focused offline-checker regression
passes for local named types, incompatible defaults, and deferred external types.
A subsequent activation regression found
pack metadata selecting its configuration schema before candidate types existed.
Deployment pack writes now use a savepoint, preflight the candidate graph, compile
the candidate configuration schema, and update its reference edges. Ordinary
pack writes still select the installed registry. The next regression reached
sensor loading and exposed a metadata-only trigger-link update using the installed
registry. That update now reuses the candidate view. The corrected registration
regression passes, including the configuration migration rollback checks.
It now also checks that an independently valid candidate with stricter named
configuration constraints rejects incompatible preserved values and rolls back
pack metadata, configuration, action schemas, and runtime bindings together.
The rebuilt service images are from project `attune-e2e-schema-volume-250f7f0e2ad3`.
Its first service run stopped during E2E setup because a helper retained an unused
import of the superseded inquiry response request model. That import is removed;
the remaining generated-client helper imports resolve. The managed sensor probe
is rerunning against the rebuilt, source-equivalent images. Its next run verified
secret stdin configuration, initial rule defaults, and protected log delivery,
then caught a generation change caused by a probe crash. The event endpoint
rejected its obsolete caller-supplied `source` field with 422. The server derives
source identity from the token; the probe and Python/JavaScript/Java SDK emitters
now omit that field. The live-generation assertion remains unchanged. Java's
emitter also no longer sends `rule_ref`; explicit broadcast omits the numeric
target, while targeting is the default when a rule is supplied. Its lifecycle
storage and polling keys now retain 64-bit rule IDs and reject fractional,
non-positive, and overflowing IDs rather than truncating them.

Pack/sensor configuration, queue payload, inquiry response, and enforcement detail
now use the shared bounded display. Enforcement detail exposes its stored input
contract without executable metadata and the original payload contract when the
event binding remains available. It no longer fetches current action/trigger
schemas. All 386 web tests pass after client regeneration, including nested named-value masking, use-site
titles/positions, prototype-named properties, and admitted enforcement displays.
The remaining rule-selector display now uses a bounded authoring preview with
values hidden until resolution succeeds; both new tests pass. Web typechecking
also passes with that final display change.

The managed secret lifecycle fixture passes against the rebuilt service images.
It verifies stdin configuration and initial rules, public defaults, live delivery
without a generation change, rotation, restart, historical disclosure, and event/
audit redaction. Full service logs contain no synthetic sentinel with both mirrors
enabled. A supervisor-driven protected-log retention check is added and awaits
validation. The successful owned project is `attune-e2e-schema-volume-51517fb6c825`;
its diagnostic evidence is retained and the stack was torn down. Sensitive startup
no longer has the temporary rollout gate. Unowned consumers select
public installed root types, retain reference edges without inventing pack
ownership, and use the same policy at event/queue/rule acceptance. A forward
migration preserves their edges through nullable-pack identities. Key assignments
and generic previews retain their separate unrestricted root-reference policy.
The owned versioned S3 harness is available at port 59073. Its archive distribution
test across API replicas passes. The broader API registration suite passed 54 of
56 tests, with the fixed named-queue blocker and an asynchronous audit deadline
failure remaining to rerun. Both API configuration binding tests pass. Final
The workspace all-targets check passes without warnings. SQLx preparation also
completes, reporting no compile-time query macros to prepare. Deployment regression
reruns and service coverage are unfinished. Web, Python, JavaScript,
and Java clients have been regenerated. E2E Python client generation also completes
from the current source export with 209 documented paths.
The SDK checks pass with
106 Python tests, 69 JavaScript tests plus TypeScript checking, and 44 Java tests.
The Python generator retains its existing limitation for gzip archive responses;
the schema and enforcement models generate successfully. No changes are committed.

The first broad Tier 3 run passed three cases then failed the timer cancellation
execution deadline. The unchanged isolated cancellation case passes, as does the
four-case date-timer/cancellation sequence. No timer behavior or deadline was
changed to obtain those results. The full 66-case non-performance selection is
not yet certified. The next broad run passed 12 cases, including the timers and
webhook cases, then failed parallel workflow enforcement. Its API-created companion
action was incorrectly marked pack-managed without a released executable snapshot.
API-created workflow actions now use the same ad-hoc admission path as ordinary
API-created actions. A workflow API regression checks snapshot resolution, and
fresh service images are rebuilding for parallel-workflow and protected-log
retention validation. The rebuilt workflow run admits the root and completes its
first child, then stalls before advancement. The focused case is rerunning with
private database wait diagnostics. Protected-log retention is running separately.
The next focused workflow run passes with unchanged deadlines, completing the
root and all three children. A fresh full Tier 3 build includes the callback fix;
the earlier advancement timeouts remain recorded rather than treated as fixed by
the isolated pass. Generated-client freshness passes against a current-source
OpenAPI export.
The lifecycle-only rerun passes on the new images, but did not execute retention:
the newly added retention block had been inserted into the adjacent SDK-log case.
It is now in the lifecycle case that owns the historical version and configuration
fixtures. Both sensor cases are rerunning; retention remains uncertified.
The paired run passes SDK Key-log authorization and all lifecycle/disclosure
steps, then rejects the retention update with 422 because the test supplied bare
retention values instead of tagged `set` patches. The request now uses the existing
API patch contract; the focused retention case is rerunning.
The latest all-target workspace compilation passes without compiler warnings.
All four configuration repository tests and all three schema-consumer repository
tests pass, including graph-only workflow validation and outer rollback scope.
The workflow API suite passed ten cases and exposed three stale cross-pack refs
plus the new regression's missing fixture release. Those fixtures are corrected;
All 14 workflow API tests and all 55 non-S3 registration tests now pass without
relaxed admission policy. This includes named-component registration, preserved
configuration rollback, system-owned rule authority, and the formerly intermittent
constrained-pack audit case. The S3-only case and broader schema/Key/configuration/
queue/webhook/inquiry API suites are rerunning. The S3-only registration case now
passes against the owned versioned storage harness, so all 56 registration cases
pass across the ordinary and explicit storage-prerequisite lanes.
The final web rerun passes all 386 tests across 64 files and TypeScript checking.
Final SQLx preparation completes its all-target workspace check successfully;
it reports `no queries found` because this workspace has no compile-time query
macros to export. Generated web-client freshness is being checked against a new
OpenAPI export from the current source.
Runtime action/event/inquiry bindings, schema-type repositories, sensor workload
and admission repositories now pass all 37 selected tests against the current
source, including partition repair, immutable contracts, captured sensor grants,
and SDK origin budgets.
The final caller audit found primitive-only flat forms still used a local renderer.
`ParamSchemaForm` now always delegates to the shared contract renderer, including
server-resolved previews for primitive fields and stored graphs for historical
records. Regressions cover preview annotations, explicit false/zero/empty values,
and secret controls under stored contracts. The inquiry form fixture now carries
its stored response graph. All 388 web tests across 65 files pass after this
change, as does TypeScript checking.
The broader inquiry suite exposed callback ingress reading adapter settings from
the executable snapshot after admission moved configuration into separate storage.
Ingress now reads the public configuration under the authenticated workload's
exact generation. Its fixture performs schema admission and verifies configuration
is absent from executable metadata. The broader API suite is rerunning without
fail-fast so other boundary results remain visible if a callback case fails.
The post-callback all-target workspace compilation passes without compiler warnings.
The broader API rerun passes all 47 selected configuration, inquiry/callback,
Key-use, schema-discovery, and webhook-security cases. Queue coverage passes seven
cases; two stale fixtures used action refs outside their generated owning packs.
Those refs are corrected and the complete nine-case queue suite is rerunning.
All nine queue API cases now pass. The shared renderer also preserves caller
template namespaces; all 389 web tests and TypeScript checking pass.
The corrected retention request reaches a real gap: generation admission kept
release-time retention metadata despite current sensor overrides. Admission now
captures current log/artifact retention settings into the new generation, while
existing generations retain their original settings. The repository regression
passes. Its unused import warning is fixed, and the post-retention all-target
workspace check passes without warnings. The rebuilt positive service check is
still running.
The fresh broad Tier 3 run passes 27 cases, including parallel/conditional/nested
workflows and chained webhooks, before a stale parameter-test expectation fails.
The API correctly rejects missing required input synchronously. The four parameter
cases now assert strict pre-creation rejection with no persisted execution, actual
stdin delivery for accepted values, retained extra fields, defaults, and explicit
empty/zero/false preservation. All four focused service cases pass using a pack-local
Python stdin probe. The broader Tier 3 selection is rerunning on callback-current
images, excluding only the lifecycle/retention case undergoing its own rebuild.
That rerun passes 13 cases then stalls during conditional workflow dispatch.
Private database waits identify cross-connection schema admission: a child holds
shared graph admission, reclamation queues for exclusive admission, and child
input resolution requests shared admission on another connection. The child cannot
commit while waiting for that second connection. A queued-reclamation regression
now exercises the ordinary child-input resolution path before applying its fix.
The regression fails on the original path at its five-second resolution deadline.
Child dispatch now uses its existing transaction for restored parameters and
pack/Key input resolution. The regression rerun, workspace check, and fresh full
Tier 3 build include this lock fix and the retention fix.
The queued-reclamation regression now passes, and workspace checking remains
warning-free. Rebuilt retention still fails after capturing the override. The
supervisor's version-count predicate only selects shared-file logs; sensor
object-segment logs therefore wait for a future seal-triggered cleanup after a
policy change. A repository regression now lowers retention with sealed history
and an active stream to verify cleanup eligibility without deleting active data.
All 83 non-broker scheduler tests pass, including the queued-reclamation regression;
six explicitly broker-only cache protocol cases were not provisioned in this run.
The object-log regression fails against the original supervisor predicate.
Version-count cleanup now includes sealed object-segment logs as well as shared
files; pending object streams remain excluded. The full artifact repository suite,
workspace check, and another full Tier 3 build include both retention fixes and
the workflow lock fix. Earlier service builds do not include this final predicate.
All 40 artifact repository tests now pass, including the policy-override regression
that preserves the newest ready version and active pending stream.
The subsequent all-target workspace check is blocked by an unrelated untracked
performance test, `crates/api/tests/artifact_read_performance_tests.rs:530`, whose
debug block references an undefined `retained`. That unfamiliar file is preserved.
Workspace library/binary checking is running separately; this is not a substitute
for the required final all-target check. Library/binary checking now passes without
warnings. The concurrent performance-test file has changed, so all-target checking
is rerunning without edits to that file.
The final all-target workspace rerun passes without warnings. The unrelated
compile blocker is resolved by the concurrent work; no edits were made here.
The earlier full service build, which lacks the final object-log retention
predicate, passes 33 cases before the timeout-notification workflow stalls.
No graph-lock wait is present. Its executor log reports schema validation failure
before child creation: absent parent configuration is restored as null and then
validated as a flat object. A focused regression distinguishes absent parameters
from explicit null before changing child-input normalization.
The empty-input regression reproduces the error. Rendering now normalizes absent
parent configuration to an empty flat map while preserving explicit null for
strict validation. Scheduler tests, workspace checking, and a fresh full Tier 3
build include the fix. The preceding service build also stops at the same timeout
case after 33 passes; its images are being reused for the other cases so this
known failure does not hide later acceptance results or protected-log retention.
All-target workspace checking passes without warnings after the empty-input fix.
All 84 non-broker scheduler cases pass after that fix, including explicit-null
rejection and empty absent configuration. The same six broker-only cases remain
outside this direct database run.
The selection excluding only timeout notification passes 44 cases, including
live WebSocket/inquiry notifications, then finds an HTTP 404 test expecting
completion although the core script exits nonzero. Its expected status is corrected
to failed without removing the 404/result assertions. Focused HTTP and protected
sensor retention service runs now use images containing both retention fixes;
the full rebuild separately includes the empty-input fix.
The HTTP rerun confirms the failed-execution envelope stores its response JSON in
`result.stdout`. The test now checks exit code 1 and parses stdout before asserting
404 and `success: false`. The broader selection excluding timeout is rerunning.
The retention capture verifies limit 1 is applied but shows only the original log
is sealed/ready; later versions are pending streams. The fixture now rotates the
restarted generation and waits for a newer ready version before lowering retention.
This preserves the rule that active pending streams cannot displace retained ready
history. Its focused service check is rerunning.
The next broad selection passes 45 cases, including the corrected HTTP error
case, then finds an obsolete secret-delivery fixture. It created a Key but supplied
no parameters and expected an automatic shell variable. The fixture now supplies
an encrypted Key template to a secret-marked flat parameter. A pack-local Python
probe reads stdin JSON, checks a digest, and verifies no environment value contains
the credential without printing it. Focused service validation is running.
The retention fixture next times out on restarted-bootstrap visibility, before
reclamation. Its rotation-stop predicate could match an older no-rotation record.
The update now carries a distinct public count and waits for that exact live value
before restart, so old records cannot satisfy the synchronization predicate.
The focused stdin probe first exceeds its deadline during cold Python dependency
setup, then fails because manual input templates are literal values (Key selectors
are resolved at rule/workflow boundaries, not manual execution). The manual probe
now submits a synthetic literal to its secret-marked parameter. Its rerun includes
the parameter-contract cases first to exercise Python delivery before this security
case; execution deadlines are unchanged. The previous Key-template fixture claim
does not establish successful manual Key resolution.
The synchronized retention fixture reaches reclamation but still fails on reused
`ba88345a1355` images. Direct inspection of that Supervisor binary finds the original
shared-file-only SQL and not the sealed-object predicate. Earlier statements that
these reused images contained the final predicate were incorrect. A fresh focused
build is running; positive service acceptance requires checking the built binary
as well as the source/test results.
The older full rebuild finishes but fails PostgreSQL startup before testing; its
Supervisor binary also contains the old predicate. The Docker builder's shared
Cargo target cache can retain artifacts newer than copied source timestamps.
`docker/Dockerfile.optimized` now touches local Rust source before Cargo compilation
to force local-crate freshness checks while retaining dependency caches. A new full
Tier 3 build includes this change. The stdin probe meanwhile completes successfully
after the four parameter cases; its assertions now require protected stdout rather
than public marker text. The probe itself rejects wrong credentials or environment
leaks via nonzero exit. Full service acceptance remains pending.
The focused `1cc457ac3229` build also fails reclamation and direct binary inspection
confirms the old predicate remains. It began before the Docker source-freshness
change. The post-change full build is the next candidate; do not treat these earlier
image rebuilds as verification of the new Supervisor predicate.
The post-freshness build `db914c1f2172` contains the sealed-object predicate by
direct binary inspection. Full Tier 3 passes 54 cases, including timeout workflows
and the protected stdin probe, then stops at a trigger fixture using `param_schema`
for payload secrecy. Both payload fixtures now use `out_schema`. The rule-mapping
fixture also rejects event selectors for secret inputs and uses a whole Key
selector with recorded-origin Key read/decrypt for authorized disclosure. Focused
event/retention and full Tier 3 reruns use the verified images and current fixtures.
Both focused cases pass on verified images (`70b0488de31e`): event payloads require
explicit decrypt access, and the sensor lifecycle exercises stdin/live updates,
restart, historical authorization, rotation, and Supervisor reclamation. The old
protected log returns 404 after cleanup; authorized current logs remain readable.
The synthetic SDK Key is absent from complete mirrored sensor service logs.
The subsequent `cargo check --all-targets --workspace -j 1` passes without warnings.
The full rerun reaches 55 passes before the new negative rule-selector assertion
expects 400 but receives 422. It now requires 422 and a Key-specific diagnostic;
the focused operational-metadata and sensor-security selection is rerunning.
The rerun confirms negative selector rejection and valid Key-backed rule creation,
then exposes an invalid newly added reader grant (`filters` instead of
`constraints.refs`). The fixture grant is corrected and focused coverage reruns.
All four focused operational-metadata and sensor-security cases now pass
(`f7c3a63ab6d1`), including recorded Key-origin disclosure, SDK log authorization,
and historical protected-log reclamation. The complete non-performance Tier 3
selection is rerunning with these corrected fixtures and verified images.
Secret-bearing sensor startup passes service-level coverage; supervisor-driven
protected-log retention still awaits its positive regression.

## Goals

- Let packs define and deploy reusable value schemas with namespaced refs such
  as `example.address` and `example.severity`.
- Allow those types wherever a schema node can declare a primitive, object,
  array, or other supported JSON Schema shape.
- Use the same resolved contract for validation, defaults, secret handling,
  dynamic forms, and schema-derived displays.
- Preserve Attune's flat per-field `param_schema`, `out_schema`, and
  `conf_schema` documents and flat execution parameters.
- Prevent specification drift within a runtime record by materializing its
  schema contract when the record is created.
- Reject recursive schema references in the initial implementation.

## Original implementation baseline

| Area | Relevant files | Current behavior / implication |
|---|---|---|
| Schema storage | `crates/common/src/models.rs` | Schema values are generally untyped JSON. There is no named-type registry. |
| API parameter validation | `crates/api/src/validation/params.rs` | Uses the common compiler; rule definitions defer templates, while queue/inquiry acceptance uses strict validation. |
| Inquiry options | `crates/common/src/inquiry_options.rs` | Uses the common compiler and strict validation for stored response options. |
| Pack checking | `crates/common/src/pack_check.rs`, `crates/common/src/pack_format/source.rs` | Shares common inline-definition validation, including nested constraints. Registry-backed named graph validation remains unimplemented. |
| Component loading | `crates/common/src/pack_registry/loader.rs` | Transactional loading with ownership preflight and component reconciliation. Pack configuration metadata also has handling outside this loader. |
| Component lifecycle | `crates/common/src/repositories/component_lifecycle.rs`, `crates/common/src/platform_catalog.rs` | Tracks managed projections, release ownership, and retirement. Types must join this lifecycle. |
| Immutable releases | `crates/common/src/repositories/pack_release.rs`, `crates/common/src/repositories/executable_snapshot.rs` | Captures executable action, runtime, workflow, and sensor metadata. Runtime schema binding must respect the selected executable release. |
| Defaults | `crates/executor/src/scheduler.rs` | Applies top-level parameter defaults; nested semantics are not centralized. |
| Secret handling | `crates/common/src/secret_values.rs` | Traverses schema properties directly, but schema secret-path collection does not traverse array items or composition branches. |
| Dynamic forms | `web/src/components/common/ParamSchemaForm.tsx` | Supports primitives and string enums; objects and arrays currently render as JSON textareas. Client validation is a limited implementation, not full JSON Schema validation. |
| Schema authoring | `web/src/components/common/SchemaBuilder.tsx` | Rebuilds schemas from a limited set of fields, potentially losing unsupported structure during edits. |
| Keys | `crates/common/src/models.rs`, `crates/api/src/dto/key.rs`, `crates/api/src/routes/keys.rs`, `web/src/pages/keys/KeyCreateModal.tsx`, `web/src/pages/keys/KeyEditModal.tsx` | Keys accept JSON values but have no value-schema binding. Their forms use independent format controls. |

Validation coverage is uneven. Rule parameters, queue items, and inquiry
options have validation paths. The execution-create and pack-config update
paths inspected do not perform equivalent schema conformance checks. Action
output schemas participate in secret redaction, but the worker success path
inspected does not enforce output conformance.

Reference support and broader enforcement are separate deliverables. A named
schema must behave like its equivalent inline schema; using a name must not
accidentally introduce a different validation policy.

## Authoring contract

### Pack definitions

Add `types/*.yaml` as a pack component directory:

```yaml
# types/severity.yaml
ref: example.severity
label: Severity
schema:
  type: string
  enum: [info, warning, critical]
```

```yaml
# types/address.yaml
ref: example.address
label: Address
schema:
  type: object
  properties:
    street:
      type: string
    city:
      type: string
    country:
      type: string
      minLength: 2
      maxLength: 2
  required: [street, city, country]
  additionalProperties: false
```

A named type describes one value, not an outer flat field map. Its body uses
JSON Schema node semantics plus supported Attune annotations. Existing
`param_schema`, `out_schema`, and `conf_schema` fields remain flat maps.

### References

Use a namespaced reference in `type` as shorthand:

```yaml
parameters:
  destination:
    type: example.address
    required: true
  severity:
    type: example.severity
    default: info
  alternate_destinations:
    type: array
    items:
      type: example.address
```

The compiler must distinguish occurrence-level `required: true` in a flat field
map from `required: [...]` inside an object value schema. This cannot be handled
by indiscriminately merging JSON objects.

Initial reference rules:

1. Use fully qualified `pack.type_name` refs; do not add contextual short-name
   lookup initially.
2. Resolve references only in schema-bearing positions, including properties,
   items, supported composition branches, and local definitions. Do not traverse
   defaults, examples, or arbitrary data as schemas.
3. Missing, inaccessible, invalid, or retired references fail closed. Never
   substitute an unconstrained schema.
4. Do not fetch arbitrary HTTP or filesystem references during compilation or
   validation. Supported local JSON Schema references must remain local.
5. Initially allow use-site annotations such as description, default, position,
   occurrence requiredness, and additional secret marking. Reject structural
   overrides rather than inventing ambiguous merge semantics.
6. A use site cannot remove inherited secret marking. Annotation inheritance and
   precedence must be explicit, and defaults must satisfy the resolved schema.
7. Cross-pack references require declared pack dependencies and an allowed
   reference policy. Schema access does not grant Key access or execution rights.

Internally, ordinary JSON Schema `$defs` and references may represent shared
subschemas. The author-facing shorthand need not be a custom URI.

## Shared schema architecture

Introduce a dedicated common schema module rather than growing the existing
database-oriented `crates/common/src/schema.rs`.

The common layer should support:

- Flat schema documents and single-value schemas as distinct contexts.
- Definition validation and schema-aware reference discovery.
- Resolution against an explicit registry/release view supplied through the
  repository layer.
- Cycle detection, depth/size limits, and deterministic compilation.
- An annotated resolved contract for forms, defaults, and secret handling.
- A standards-compliant JSON Schema for the validator.
- A dependency manifest recording refs, immutable revision identities, and
  content digests.
- Shared default application and validation utilities.
- Sanitized errors containing field locations without disclosing secret values.

Attune annotations must remain available to runtime consumers even when stripped
from the validator schema. Defaults are an explicit Attune operation; JSON
Schema validation itself does not populate them.

Keep authored references separate from resolved contracts. UI editing must not
replace named references with expanded definitions merely because it fetched a
rendering schema.

## Binding time and drift prevention

### Decision: materialize runtime contracts at record creation

Resolve and snapshot the applicable schemas when ephemeral runtime records are
created, rather than requiring every schema consumer to be permanently bound
to external type revisions at pack deployment.

Deployment still validates the candidate schema graph, references, ownership,
and compatibility with applicable configuration. It preserves authored refs
and immutable type revision bodies. Runtime creation makes the authoritative
selection of the schema contract for that record.

This allows newly created records to use newly available external type
definitions while existing records retain the contract under which they were
accepted. It avoids runtime lookup against mutable registry state.

### Source selection

Runtime binding must not undo immutable executable-release selection:

- For an execution, choose the executable snapshot first. Resolve same-pack
  types from that selected release, not a newer release containing different
  code and type definitions.
- Manual execution creation always selects the latest eligible contract on the
  server. Clients cannot select an older binding, type revision, schema, or
  executable release. "Latest" remains aligned with the selected executable
  release for same-pack types; external selection must satisfy its dependencies.
- Rule-created executions inherit the binding materialized on their enforcement.
  The enforcement selects executable and schema sources before rendering or
  persisting parameters. Execution creation does not choose newer types again.
- Resolve external types from an explicit permitted revision view at runtime
  creation. The default can be the active external release at that boundary;
  dependency/version constraints, if supported by the existing dependency
  contract, must still apply.
- For an event, choose and record the applicable trigger schema revision at
  ingestion. When ingestion is tied to a pinned sensor release, define that
  source relationship explicitly rather than mixing incompatible revisions.
- Reject a source definition that no longer satisfies the declared dependency
  or schema contract instead of silently selecting another definition.

These rules should be finalized before persistence changes. Binding at runtime
prevents intra-record drift, but does not by itself prove that a newly changed
external contract is compatible with older action code.

### Persistence and atomicity

Store an immutable binding containing:

- The source component/release identity.
- The authored schema or an unambiguous link to its immutable source.
- The resolved annotated schema or a self-contained schema bundle.
- Referenced type revision identities and digests.
- A deterministic binding digest and compiler contract identifier.

Select revisions, validate supplied values, derive secret handling, and persist
the record and its binding under one consistent transaction/view. Concurrent
type updates must not produce a mixed binding. Publish runtime notifications
only through the existing successful record-creation flow.

Use repositories for every database operation. Whether the bundle is embedded
in a runtime record or stored as a deduplicated immutable object is an
implementation decision; it must remain available for the record's lifetime.
Avoid a mandatory live lookup into mutable component rows.

### Record-specific boundaries

| Record | Binding boundary | Subsequent behavior |
|---|---|---|
| Enforcement | Creation after executable selection, before parameter rendering, validation, redaction, and insert | Rendered parameters and encrypted values use one fixed action input/output binding. Its execution inherits that binding and recorded secret origins. |
| Execution | Manual creation selects the latest eligible contract after executable selection, before parameter persistence/redaction; rule creation inherits its enforcement binding | Defaults, rendered input validation, output validation, secret handling, and displays use the binding throughout the execution. No manual contract selection is supported. |
| Workflow execution | Root workflow execution creation, before input/default processing | Workflow input/output schemas remain fixed across scheduling, retries, waits, and resume. Reuse the root execution binding where possible rather than creating competing bindings. |
| Workflow child execution | Child creation under the parent workflow's applicable pinned contract | For statically identified tasks, capture or pin their schema sources with the workflow contract. Dynamically selected actions need an explicit child-creation binding boundary and must retain that binding afterward. |
| Event | Ingestion before validation, redaction, and insert | Events remain immutable. Rule processing and historical display must not reinterpret an event using a newer trigger schema. |
| Inquiry | Creation before validating stored response options | Responses and option selection use the original response-schema binding. |
| Queue item | Admission | Retain the admission contract for payload updates, retries, and dispatch. New items use the latest eligible schema. No item rebind operation is provided initially. |

### Updates do not imply rebinding

The event section separation is confirmed. Trigger `param_schema` describes rule
trigger-instance parameters, `out_schema` describes emitted payloads, and optional
flat `event_config_schema` describes sensor-supplied event configuration. Missing
optional configuration stays absent; supplied sections validate strictly after
public defaults. Webhook metadata does not use either author-supplied configuration
schema. Its server-owned contract protects the webhook key and all supplied headers.

Event contract cleanup cannot run from parent row-delete triggers because native
DEFAULT repair uses DELETE/INSERT before attaching the replacement leaf. Exact
repository deletions clean owner IDs atomically. Partition and boundary retention
scan an indexed, bounded batch of eligible bindings before checking missing owners.

Re-evaluate updated values against the stored binding. Ordinary execution status
changes, output persistence, workflow checkpoints, retries, and resume must not
resolve the latest named types again.

For example, changing an execution from `running` to `completed` must not cause
its result to be checked against a type introduced after the execution started.

If an API permits changing a schema-bearing record's contract, make rebinding
explicit and authorized. Validate the complete effective value, persist the new
binding atomically, and record its digest transition without payload secrets.
Do not rebind in-flight executions or immutable events. A new execution may
select a new contract; a retry within an existing execution retains its contract.

### Persistent definitions and values

Keys and pack configuration are not ephemeral runtime records. Their policy
needs separate handling:

- Add an optional single-value `value_schema` to Keys, supporting inline and
  named types through the same compiler. Bind on assignment or explicit schema
  change; ordinary value updates validate against the existing binding unless
  an explicit rebind is requested.
- Validate Key plaintext before encryption. Schema changes without replacement
  values must respect existing read/decrypt authorization when inspecting stored
  material. Schemas do not grant disclosure authority or change encryption rules.
- Bind pack configuration to its effective configuration schema and validate
  writes. Revalidate preserved config when deployment changes that schema; define
  the treatment of newly required fields explicitly rather than silently
  invalidating installed configuration.
- Forms for a pending submission may use a current preview binding. Backend
  acceptance is authoritative. A preview digest can detect contract changes
  between form loading and submission and require refresh instead of surprising
  the user.

## Recursion and resource limits

Reject all recursive references initially, including optional recursion.
Detect direct and indirect cycles through named types and supported local
references, including cycles hidden under composition and array items.

Diagnostics should identify the chain and source location:

```text
example.person -> example.organization -> example.person
at schema.properties.employer
```

Optional recursion is not safe merely because the form is lazy: compilation,
defaults, schema display, and secret traversal may still eagerly follow it.
Supporting it later requires coordinated bounded graph-aware behavior in every
consumer and should be a separate proposal.

Even acyclic graphs can expand exponentially. Limit dependency depth, source
and compiled sizes, and rendered nodes. Preserve shared definitions internally;
do not blindly copy complete trees at every reference.

Optional objects should remain absent until provided, defaulted according to
the explicit default policy, or added by the user. Do not instantiate an entire
object tree merely to render a form.

## Validation, templates, and defaults

Separate two modes:

1. Authoring-time validation checks literals while deferring symbolic template
   values. Whole-object templates must not fail because a fabricated empty
   placeholder lacks required children.
2. Runtime validation checks fully rendered values against the stored binding
   before delivery or acceptance.

The existing placeholder approach is insufficient for constrained named objects,
patterns, and complex branches. Refactor it rather than adding more sample-value
generation heuristics.

Centralize defaults and define behavior for missing values, explicit `null`,
empty strings, `false`, and `0`. Nested defaults must not create absent optional
object trees automatically unless that behavior is explicitly part of the
contract. Referenced defaults and use-site defaults must be validated.

Define validation targets before enforcing action outputs: parsed action payloads
are distinct from Attune's execution-result envelope. Define JSONL record versus
aggregate validation and workflow `output_map` validation explicitly.

## Secret handling

Secret values are text or nested lists/objects with text leaves. Numeric,
boolean, and null secret types are not supported. Secret nodes and their
descendants cannot declare `const`, `enum`, defaults, or examples. Named/local
references and composition cannot bypass these restrictions. Public enclosing
literals must not embed secret descendants. Concrete validation checks
unconstrained children too; malformed outputs remain secret for protection.

Schema resolution must happen before secret-path derivation and before any
runtime persistence or output mirror that could expose plaintext.

Extend secret handling to objects, array elements, and supported composition
branches. For ambiguous branches, conservative redaction is preferable to
under-redaction. Arrays require value-aware paths or an explicit path-pattern
contract; simply collecting static object-property pointers is insufficient.

The runtime binding governs secret classification for that record. Later type
updates cannot weaken historical classification or replace recorded secret
origins. Preserve the authority rules in
`docs/permissions/delegation-and-secret-disclosure.md`.

Never include secret submitted values in validator errors, audit details,
definition defaults, examples, or schema previews. Type reuse must not expand
Key read/decrypt rights or execution-token permission scope.

## Dynamic forms and authoring

Upgrade the shared form machinery, not each page independently:

- Named-type selection, inspection, and dependency diagnostics in schema editors.
- Lossless round-tripping of references, nested schemas, and unsupported keywords.
- Server-resolved annotated schemas for all schema-derived forms and displays.
- Structured object properties and arrays of primitives/objects.
- Optional object add/remove controls and bounded/lazy rendering.
- Nested defaults and field errors using consistent paths.
- Secret-aware controls and existing disclosure restrictions.
- Template-aware editing for both whole values and nested fields.
- Key creation/editing using the shared single-value form renderer.

Unsupported JSON Schema constructs can retain a raw editor with an explicit
explanation. Never silently remove their constraints or render unresolved named
types as unconstrained strings. Form/client validation is advisory; backend
validation remains authoritative.

## Coverage boundary

Initial coverage must include:

- Action input/output schemas and execution forms.
- Workflow input/output schemas, task action forms, and output maps.
- Trigger schemas and event ingestion/processing.
- Sensor parameter/configuration schemas.
- Pack configuration schemas and values.
- Queue item schemas and dispatch parameter forms.
- Inquiry response schemas and stored response options.
- Key value schemas.
- Schema-derived displays and curated result views.

Not every structure named `schema` is JSON Schema today. For example,
`crates/api/src/dashboard_data/contracts.rs` defines a dashboard source parameter
contract as required/optional name lists, with custom route validation. Named
types there require a separate contract refactor, not merely a resolver call.

Do not automatically replace native Rust platform configuration types, the pack
archive format's schema-version marker, database schema identifiers, or add
schemas to currently schema-less storage. Track any additional genuine
user-authored value-schema surfaces discovered during implementation.

## Implementation sequence

### Phase 1: contract and common compiler

- Finalize the node/document distinction, reference grammar, annotation rules,
  supported JSON Schema vocabulary, and runtime binding policy.
- Implement deterministic resolution, dependency manifests, cycle/resource
  checks, annotated contracts, and JSON Schema compilation.
- Centralize defaults and validation helpers.
- Replace duplicated flat converters and align pack checking/format checking.
- Cover canonical metadata and authoring field names consistently across entry
  points; do not introduce a second format version or compatibility adapter.

Gate: named and equivalent inline schemas have identical behavior, with
deterministic errors for missing refs, cycles, and unsupported overrides.

### Phase 2: registered type component

- Add models, repositories, migrations, and immutable type revision bodies.
- Integrate `types/*.yaml` into ownership preflight, component loading,
  projections, release stamping, retirement, and removal policy.
- Validate the complete candidate graph before mutating consuming components,
  including pack configuration outside the component loader.
- Add reverse-dependency inspection and safe removal rules. Preserve revisions
  needed by live runtime bindings even after definitions retire.
- Provide authenticated list/get/inspect API and CLI/MCP discovery. Initially
  keep definition mutations pack-managed.

Gate: malformed type definitions roll back registration, and type lifecycle
cannot invalidate an already materialized runtime binding.

### Phase 3: runtime binding and persistence

- Select schema sources consistently with executable and sensor release pins.
- Materialize execution/workflow/event/inquiry contracts at their creation
  boundaries using repository-owned transactional operations.
- Define workflow child and queue-item update boundaries explicitly.
- Use stored bindings on updates, retries, resume, output persistence, and
  historical display; never resolve current registry state in those paths.
- Add explicit rebinding only to appropriate mutable schema-bearing records.
- Ensure binding availability and cleanup follow runtime retention. Event
  storage must remain immutable and retain existing partition/ID semantics.
- If mutable execution columns are added, update history-trigger change checks
  and use digest summaries for large JSONB bindings.

Gate: concurrent type updates cannot produce mixed contracts, and new records
may select newer external types without changing existing records.

### Phase 4: backend consumers and security

- Route validation, defaults, redaction, secret destinations, and output handling
  through the common layer and applicable runtime bindings.
- Add Key schema storage and validated encrypted/plaintext write paths.
- Validate pack configuration writes and preserved config on schema changes.
- Close validation gaps deliberately, including rendered execution inputs,
  trigger payload contracts, and action/workflow output validation.
- Define failure behavior for each boundary. Do not change conformance policy
  accidentally based on whether a type was named or inline.

Gate: nested and array-contained secrets cannot leak through parameters,
results, runtime logs, events, validator errors, or API responses.

### Phase 5: shared UI integration

- Add named-type inspection/selection and lossless schema editing.
- Separate authored schemas from resolved rendering contracts.
- Add structured object/array rendering and optional-field controls.
- Integrate Key forms and every existing dynamic-form/display caller.
- Surface preview/acceptance binding mismatches clearly.

Gate: editing preserves refs and constraints; all form callers use the same
resolved semantics with bounded rendering and safe secret controls.

### Phase 6: verification and documentation

- Add the tests below using repository-owned disposable fixtures and the
  guidance in `docs/testing/running-tests.md`.
- Regenerate API clients, including web and other affected generated clients.
- Document type authoring, dependencies, override restrictions, runtime binding,
  updates, deletion, forms, and recursion policy.
- Update `AGENTS.md` with concise invariants/pointers if the new concept changes
  project guardrails.
- Run targeted tests, `cargo sqlx prepare` after database changes, and
  `cargo check --all-targets --workspace` with no introduced warnings.

## Acceptance tests

- Inline/named equivalence for enums, objects, arrays, nested types, and defaults.
- Missing, inaccessible, retired, duplicate, and invalid references.
- Direct, indirect, optional, array-contained, composition-contained, and
  supported local-reference cycles.
- Large acyclic graphs, depth/size limits, and shared-definition reuse.
- Registration rollback, ownership checks, type retirement, and dependencies.
- Concurrent binding/type updates and preview/submission mismatches.
- Executable-release alignment when same-pack types change with action code.
- New runtime records selecting updated external types while old records keep
  their bindings, including queued executions and long-running workflows.
- No rebinding on status updates, completion, retry, checkpoint, or resume.
- Immutable events retaining ingestion schemas after trigger/type updates.
- Workflow child, dynamic-action, inquiry, and queue payload-update boundaries.
- Literal versus template validation and strict validation after rendering.
- Defaults for absent optional objects, missing fields, `null`, `false`, and `0`.
- Sanitized validation errors and secret handling across objects, arrays,
  composition branches, results, mirrors, and logs.
- Key create/update/rebind, encryption changes, and read/decrypt authorization.
- Pack configuration preservation and newly required fields on deployment.
- Lossless UI round-tripping and every existing schema-derived form/display.

## Decisions to finalize before implementation

1. Confirm `type: pack.type_name` and `types/*.yaml` as canonical syntax.
2. Confirm no recursion and annotation-only use-site overrides initially.
3. Finalize the external revision-selection policy at runtime creation and any
   dependency/version constraint behavior without bypassing executable pins.
4. Choose binding persistence/deduplication and lifetime management.
5. Finalize workflow child/dynamic-action and pending queue-item policies.
6. Define enforcement failure behavior and output validation targets.
7. Define configuration migration/revalidation behavior for schema changes.
The enforcement handoff policy is confirmed: bind at enforcement creation and
inherit that contract when creating its execution. Manual creation uses only
the latest eligible server-selected contract, without a contract selector.

Queue-item policy is confirmed: preserve the admission binding through updates,
retries, and dispatch, without an initial item-rebind operation.

Output conformance policy is confirmed: validate parsed action data, including
each JSONL record, and record sanitized warnings rather than changing successful
execution status. An absent output schema imposes no output constraints.
Invalid sensitive output still requires conservative encryption and suppressed
mirroring; warning-only conformance does not relax secrecy.

Schema metadata is non-sensitive. Authenticated clients may list and inspect
installed schemas without additional schema-read permissions. Schema documents
must not contain credentials or other sensitive literals, including defaults
and examples. Secret values are text trees only. Compilation rejects numeric,
boolean, and null secret types and `const`, `enum`, defaults, or examples beneath
secret markers. Public enclosing literals cannot embed secret descendants.

Keys may reference any installed named schema, regardless of Key ownership,
owning-pack dependency declarations, or the type's pack-reference visibility.
Key value read/write/decrypt authorization remains unchanged. Ordinary value
updates retain their assigned contract; schema changes and explicit rebinding
validate the complete effective value. Pack-managed schema graphs still enforce
their own dependency/version and reference-policy contracts.

Configuration migration policy is confirmed: apply public defaults to preserved
configuration, then reject and roll back an incompatible deployment. Newly
required fields without defaults require replacement configuration.

Configuration secrecy policy is confirmed: encrypt secret fields, redact ordinary
reads, and require pack read plus pack decrypt authorization for plaintext reads
or template delivery. Schema inspection remains authentication-only.

Inquiry secrecy policy is confirmed: participation or ordinary visibility does
not grant plaintext secret access. Disclosure additionally requires explicit
`inquiries:decrypt` authority and recorded-origin checks. Authorized respondents
may select opaque preconfigured option handles without seeing secret values.

The recommended initial scope is pack-managed named types, a shared compiler,
runtime-record materialization, secret-safe bindings, and structured object/array
forms. Recursive types and sophisticated structural overrides are separate
follow-up proposals.
