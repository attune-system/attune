# Real non-admin interoperability probe

This is runnable source, not evidence of a passing connected deployment. Run it
only in the central acceptance lane after the source freeze is released. It
does not build binaries, install dependencies, generate clients, start services,
disable auth or change deployment gates.

## Ownership and invocation

Reuse `scripts/run-bootstrap-lifecycle-acceptance.py` and its integrated
`scripts/acceptance/fixture_support.py`. The lifecycle runner owns the stack,
images, databases, workers, volumes and teardown. These new probes are foreground
children. Do not run them against a developer stack or existing browser profile.

1. Have the owning runner start its ordinary disposable stack using its existing
   immutable-image and local Docker-context checks.
2. Supply the runner's private context on stdin to the concrete provisioner:

   ```sh
   python3 scripts/acceptance/provision_interoperability.py --execute < /owned/context.private.json
   ```

   The context requires the existing runner fields `run_id`, `project`,
   `owner_label`, `compose_file`, `api_url` and `operator_credential_file`. The
   existing ordinary operator password credential must explicitly name this
   owned project. There is no first-admin creation or seed shortcut.

   The provisioner creates five distinct local identities and their scoped
   permission sets using public APIs. It writes private separate credentials and
   `g-interop-principals.private.json` under the owned Compose directory. No
   operator credential is used by the acceptance invoker or installer.

3. Use E/D's canonical pack archive helper to prepare the fixture below, using the
   actual returned sponsor Key ref. This must use the real images, runtimes and
   admission path. Missing image, runtime or native inputs must fail setup. The
   probe contains no archive mock or "skip tests" option.
4. Merge the provisioner's binding with the artifact/fixture fields below into a
   new mode-0600 file. Add `interoperability_binding_file` to the owning context.
5. Await the concrete probe before stopping application writers:

   ```sh
   python3 scripts/acceptance/interoperability_probe.py --execute < /owned/context.private.json
   ```

   This is a separate foreground probe stage, not an automatic change to E's
   lifecycle driver. Central must wire and await it. `project` must start with
   `eaccept-<run_id>-`. Every service in the private JSON Compose file must carry
   `io.attune.acceptance.owner=<run_id>` and an actual immutable image reference.

## Principal separation

| Principal | Actual role |
| --- | --- |
| deployer | Pack-scoped read/install/configure only. No Key, identity or permission-management grants. |
| runtimeconsumer | Execute/read the fixture actions, read/cancel own executions, create items only in the two owned queues. No internal permission-set membership or Key/permission/identity grants. |
| reader | Pack/action metadata read only; real execute request must return 403. |
| sponsor | Own-source component metadata read, create items in the two owned queues, and create/read/decrypt only its own identity-owned source Key. Issues exact runtime and input-only consent. |
| maintenance | Own-pack configure/decrypt/read, own-action update, own-queue read/update and item read/decrypt/delete, own-pack execution read/decrypt/cancel, ordinary permission management and identity update. Explicit read/decrypt of the one actual source Key is added only for the controlled result witness. Never substitutes for an installer, invoker or sponsor. |

Identity mutation and permission-management routes currently authorize without a
target context. Those maintenance grants are therefore unscoped for this
disposable deployment. This limitation is explicit, not an admin-role fallback.
The probe checks effective grants and assignments again through `/auth/me`, and
checks the actual source Key's owner ID against the sponsor identity.

## Fixture contract

Pack ref is exactly `ginterop_<run_id with hyphens replaced by underscores>`.
The owning archive helper must supply:

- `<pack>.probe`, an ordinary action running the exact
  `scripts/acceptance/interoperability_worker.py` source. The archive must contain
  exactly one regular file ending `/interoperability_worker.py` with the measured
  source hash. Use stdin JSON parameters, not environment parameter injection.
- `<pack>.input_probe`, running that same entrypoint with one retained typed
  default input declaration reading the actual encrypted identity-owned source
  Key returned by the provisioner. Exactly one target per invocation variant is expected.
  The input is a string in parameter `input`; never serialize its plaintext into
  the archive, binding or evidence. Use the source ref in the authored expression.
- Both actions accept public string parameters `nonce` and `runtime_action_ref`,
  labelled `Probe nonce` and `Runtime action reference` for the real form. Use
  `<pack>.probe` as the runtime read target.
- A nonempty pack-local SDK permission default allowing that pack-local action
  read. Do not assign this permission set to any human principal. The selected
  worker performs a real authenticated GET of that action and returns only its
  HTTP status. Empty-ref executions return only a boolean saying the general
  token is absent; they do not inspect or disclose JWT contents.
- A declared structured output accepting the probe's JSON receipt: `protocol`,
  `nonce`, `api_token_present`, nullable integer `runtime_read_status`, and optional
  `input_sha256` and `runtime_value_sha256`. Use D's canonical flat schema authoring, not JSON Schema arrays
  or guessed runtime metadata. Configure the actual runtime's JSON output parser
  so the saved execution's `result.data` is the exact receipt object, not a
  guessed text/stdout parser fallback.
- A public integer pack configuration field `interop_counter` with a default
  other than 77, a named type `<pack>.probe_receipt`, an idle managed queue
  `<pack>.inbox`, and an editable pack permission set for export/import. The queue
  must have no other producer or pending work during config roundtrips.
- `<pack>.runtime` and `<pack>.runtime_alt` are distinct pack-local sets allowing
  the probe's pack-local action read. `<pack>.external_runtime` initially has the
  same harmless pack-local grant. It is changed through ordinary canonical
  definition import only after the actual source-owner runtime approval is issued.
- `<pack>.external_probe` runs `interoperability_worker.py`, accepts `nonce` and
  public `runtime_key_ref`, and has no declared Key input or default SDK selection.
  The probe explicitly selects `[<pack>.external_runtime]`. It makes a real Key GET
  and retains only status 200 and the canonical JSON value's SHA256. Repository-side
  comparison must match the known source digest, so a redacted 200 response cannot
  masquerade as successful Decrypt. Do not assign this set to a human.
- `<pack>.input_workflow` is a real two-file workflow action with a single task
  invoking `.input_probe`. Forward `parameters.nonce` and
  `parameters.runtime_action_ref`, leave SDK refs empty, and retain the target
  action's authored Key input default. A task named `read` is suggested; the probe
  uses the actual discovered immutable task pointer, never guesses a name/index.
- `<pack>.input_rule` subscribes to `<pack>.input_tick` and invokes `.input_probe`.
  Forward `event.payload.nonce` and `event.payload.runtime_action_ref`, leave SDK
  refs empty, and retain the authored input default. The public managed trigger
  accepts only those public string fields. `<pack>.emit` runs the exact
  `interoperability_emitter.py` source with stdin JSON `trigger_ref`, `nonce` and
  `runtime_action_ref`; its admitted `.emitter` set permits Events/Create only on
  the owned trigger. No Access principal emits directly. Emitter JSON output uses
  the same receipt shape with `runtime_read_status: 201` for the real POST status.
- `<pack>.input_inbox` is an enabled, accepting, single-item queue invoking
  `.input_probe` with empty SDK refs. Forward `item.nonce` and
  `item.runtime_action_ref`; retain the authored input default. Use public string
  item-schema fields for these two values and automatic ACK on actual completion.
- `<pack>.inbox` is an accepting batch queue invoking `<pack>.batch_probe`, with
  `config.dispatch.batch_size: 2`, coalescing disabled and empty SDK refs. Its item
  schema contains required public string `nonce` and secret string `secret`.
  Action parameters use `queue_items: "{{ queue_items }}"`. `.batch_probe` runs the
  exact `interoperability_batch_worker.py` source. It takes actual item-context
  objects with `id` and `payload`, returning `protocol: interop-batch-v1`,
  `api_token_present: false`, and a sorted `items` array of `id`, `nonce` and
  `secret_sha256`. All fields are public receipt metadata; no secret payload value
  is returned or logged. Batch item order is normalized by numeric ID. Declare
  the corresponding output field map with a named object/list type as needed.

For all action entrypoints, store the canonical filename or `actions/<filename>`
in the saved executable. All three worker sources must occur exactly once in the
archive with their actual measured source hash. The inspector checks the saved
executable entrypoint too. The probe never manufactures a runtime, graph,
schema-binding ID, managed admission or delivery proof. E remains the canonical
archive/schema/runtime owner. Package the actual admitted default declaration
through the current compiler, not a plaintext secret/default literal or an ad hoc
row. The archive must yield action, workflow_task, rule and queue declarations
for `.input_probe`; the exact source paths/revisions come from real discovery.

No native/runtime placeholder is manufactured by the probe. Supply the real
canonical archive and helper's measured release digest. Upload is performed by
the limited deployer through multipart `/api/v1/packs/upload`, without force,
test suppression or server-visible local path registration.

## Private binding additions

Keep the provisioner's actual `roles`, `source_key_ref`, `input_sha256`, `api_url`,
`project` and `run_id`. Add these fields, with real paths and measurements:

```json
{
  "archive_file": "/owned/interop.tar.gz",
  "expected_release_digest": "MEASURED_CANONICAL_RELEASE_SHA256",
  "action_ref": "ginterop_RUN.probe",
  "input_action_ref": "ginterop_RUN.input_probe",
  "runtime_action_ref": "ginterop_RUN.probe",
  "queue_ref": "ginterop_RUN.inbox",
  "schema_ref": "ginterop_RUN.probe_receipt",
  "permission_ref": "ginterop_RUN.runtime",
  "default_refs": ["ginterop_RUN.runtime"],
  "config_field": "interop_counter",
  "config_default": 5,
  "inspector_binary": "/owned/bin/interoperability_inspector",
  "inspector_context_template_file": "/owned/g-runtime-observer-template.private.json",
  "inspector_release_digests": ["MEASURED_CANONICAL_RELEASE_SHA256"],
  "transports": {
    "cli": {"command": ["/owned/bin/attune"]},
    "mcp": {"command": ["/owned/bin/attune-mcp"]},
    "python": {"command": ["/owned/sdk-venv/bin/python", "/SOURCE/scripts/acceptance/interoperability_python_sdk.py"]},
    "javascript": {"command": ["/usr/bin/node", "/SOURCE/scripts/acceptance/interoperability_js_sdk.mjs", "/owned/js-sdk/dist/index.js"]},
    "java": {"command": ["/usr/bin/java", "-cp", "/owned/probe-classes:/owned/attune-sdk.jar:/owned/jackson-databind.jar:/owned/jackson-core.jar:/owned/jackson-annotations.jar", "InteroperabilityJavaSdk"]}
  },
  "browser": {
    "command": ["/usr/bin/node", "/SOURCE/scripts/acceptance/interoperability_browser.mjs", "/owned/playwright/index.mjs"],
    "executable": "/owned/chromium/chrome",
    "web_url": "http://127.0.0.1:OWNED_WEB_PORT"
  },
  "artifacts": {
    "/ABSOLUTE/ACTUAL/FILE": "MEASURED_SHA256"
  }
}
```

`artifacts` must pin each invoked binary, the canonical archive, all repo
probe/observer/scenario/worker sources checked by preflight, the provisioner and
Java source, the inspector binary and private template, the built JS SDK entrypoint, the installed
Playwright entrypoint and Chromium executable. Pin Java jars and the actual
`InteroperabilityJavaSdk.class` for each class-directory classpath entry. Also
record the complete SDK dependency/build inventory alongside central evidence;
entrypoint hashes alone are not complete SDK provenance. The Java probe source
is in this repo; compile it centrally against the actual current SDK/Jackson
jars, then invoke the compiled class without Maven or downloads. Use a centrally
installed Python SDK venv, not inherited PYTHONPATH. No SDK source files or
generated clients are copied into this repo.

The Python and JS probes use the supported typed input-approval adapters. Java
uses `AttuneClient.controlPlane(...).approvals()` and the tagged SDK input request.
Other operations use each SDK's supported public HTTP client, not urllib/fetch
substitutes inside the SDK lane. CLI uses isolated native profiles. MCP uses
actual stdio initialize/tools-call messages. A tool error fails that lane; there
is no fallback to the API probe and no lane-selection flag.

## Read-only native observer

E supplies a private template with `run_id`, `project`, `database_url`, and
`schema`, bound to the actual mapped loopback PostgreSQL endpoint and an ordinary
SELECT-only observer principal. Never use the app, database owner or superuser.
The probe writes a separate mode-0600 derived context after real upload, using the
actual pack ID, five role IDs, expected release digest and a fresh receipt key.
F's storage inspector/context is untouched. No schema/client generation is needed
for this local example.

`crates/common/examples/interoperability_inspector.rs` accepts one bounded JSON
request on stdin with `--execute`, using only
`ATTUNE_INTEROPERABILITY_INSPECTOR_CONTEXT` for its private context path. The
repository runs one bounded repeatable-read/read-only transaction, validates the
observer role, owned pack ID/ref/release and request scope, and loads SAVED
admission/policy records. It does not consume live runtime authority, require the
old human to be active, or substitute current metadata pointers.

It returns admission/policy/release IDs and digests, saved binding/selected refs,
saved executable entrypoint, exact runtime approval IDs/digests and the exact
input approval IDs from immutable input-receipt `selections`. It verifies receipt
digests and row/renderer/declaration links and follows retained receipt-source
edges. It does not infer the consumed approval from every approval for a
declaration. Queue output includes item-to-actor IDs and contributor digest/count
metadata, not usernames, grants or payloads. Each contributor's digest/body must
equal its original accepted queue-item authority, and its saved actor must match
the item's attribution. It reads saved schema contracts through
`SchemaContractRepository`, not a guessed `execution.schema_binding` column.

Worker results are separate from this metadata observer. The worker stores JSON
under `execution.result.data`; C can encrypt that whole field after runtime reads,
even if the program only printed hashes. The separate maintenance witness has
explicit scoped execution read/decrypt, queue-item read/decrypt and read/decrypt
of the one actual source Key. It calls the ordinary authorized detail endpoint
with `include_secret_values=true`, compares only the exact nonce/hash receipt in
memory, and discards config/result bodies. No plaintext, response body or tokens
are written to evidence. This is an independent output readback, not a replacement
for any native mutation or denial lane. The installer, runtimeconsumer and reader
are not elevated. The DB observer receives no encryption key and never decrypts
values. Its optional comparison operation only compares already-public saved
`result.data`; protected results are always verified through the explicit witness.

Every response echoes the exact run/project/nonce request and carries a MAC over
`[1, request, observations]`. The Python observer verifies it before accepting
metadata. Contexts, output and query counts are bounded; oversized/ambiguous
results fail rather than being silently omitted. No proofs, origins, ciphertext,
Key/config values, credentials or free-form database errors are emitted.

## Executed matrix when central runs the probe

For **every** API/CLI/MCP/Python/JS/Java lane:

1. Omitted/default, explicit empty and explicit nonempty permission selections.
   Check the actual execution ID, stored selection, real worker completion,
   schema binding and nonce-bound worker receipt. Token presence is checked only
   as a boolean. Nonempty selection must make a real permitted API read.
2. Discover the real retained declaration, issue exact input-only consent as the
   real owner, independently GET its stored purpose/issuer/target, execute the
   typed input action with an empty SDK selection, and revoke through that lane.
3. Read the installed named schema; persist queue aggregate/separate/aggregate;
   set configuration then reset to its release default; export and re-import the
   canonical authored permission definition. Each mutation is independently read
   back through the API, not accepted from command exit status alone.
4. Issue a real runtime-purpose approval, import a fixed external definition,
   execute with the exact SDK ref, replace the approval through that native lane,
   and observe the old policy's real denial. Add a harmless pack-scoped action-read
   grant through canonical import to force actual mixed-scope readmission,
   rather than claiming an identical no-op import rebuilt policy. Prove new
   execution pins the replacement ID, wait on the server's actual expiry, and
   observe another native denial. Both older execution snapshots must keep their
   original pins. This takes at least 60 seconds per lane and uses no clock/DB
   edits. Restore the original harmless definition in a finally block.
5. Repeat real declared-input issuance, dispatch, worker receipt verification and
   purpose-preserving revocation for all four invocation kinds. Workflow children,
   real execution-token-emitted rule events, and actual single-queue dispatch are
   observed by nonce and saved binding. Await root completion and item ACKs before
   revoking. Each lane also attempts direct post-input-revocation execution through
   its native transport and must return a real HTTP denial without creating a row.
6. For aggregate and separate modes, pause only the owned batch queue, enqueue
   distinct random secret items as runtimeconsumer and sponsor through that lane,
   then enable dispatch. Observe actual batch cardinality, item/actor attribution,
   executor identity, immutable contributor counts/digests and nonce/hash worker
   receipts. Aggregate must produce one mixed batch with no inherited single
   executor; separate must produce two single-requester batches. Test actual
   foreign execution/log and item reads as the limited principals and await ACKs.

Then use the real browser to issue/revoke input-only consent through the page.
Issue new persistent input consent, freeze deployer and sponsor with the separate
maintenance principal, prove their old access is denied, and repeat actual
runtime/default and typed-input/empty execution through all six lanes. The real
browser submits default, empty and explicit nonempty execution selections after the freeze. Finally
maintenance explicitly revokes input consent and the next real API input
execution must be denied before worker dispatch.

Then change action defaults through the real management API and its readmission
path. The browser must submit a fresh default execution with the new options and
rerun the original explicit-empty and explicit nonempty executions without changing
the preselected radio, option selections or parameter fields. Actual wire bodies
must preserve both `[]` and the original explicit refs rather than inherit the new
defaults. Both reruns must create distinct new execution IDs. The repository
observer must prove both old executions kept their exact saved policy/release/refs
and the new default execution retained the new policy. Finally restore defaults
and compare every saved admission and consumed
input approval ID again. This runs after the final input-revocation causal check,
so unrelated policy readmission cannot masquerade as a revocation denial.

Browser automation uses a fresh context, actual login/forms/buttons/network
responses and the installed browser. It never intercepts or mocks transport. It
asserts the exact input request, actual purpose, confirmation text and persisted
revocation. Execution checks compare actual execution-options pack authority,
policy revision and defaults, then assert the submitted HTTP body's omission or
explicit empty array. No screenshots, storage state, raw console logs, network
headers, HARs or traces are retained. An in-memory digest canary checks the DOM,
JSON responses and console for the provisioner's random source secret without
passing that plaintext to the browser script. Credential values and console
messages are not written into artifacts.

## Acceptance criteria and status

All cases above are source-implemented mandatory checks, not optional documented
gaps. Final guards require all 24 lane/binding cases, all 12 real batch cases, six
runtime replacement/expiry cases, all 18 native denial cases and the real browser
readmission/rerun case. Command/parser/network failures are not denials: CLI must
return a real HTTP envelope, MCP a native structured HTTP error, and each SDK its
own actual HTTP exception. Only bounded status/machine-code/request-ID diagnostics
are retained. Expected actor/action/policy context is labelled separately and
comes from the prior saved admission, not invented response details.

`interoperability_criteria_passed` is true only after actual execution of all
mandatory checks and successful owned cleanup. Source syntax checks and offline
tests cannot set it. Whole-SOW `full_sow_acceptance` remains false here because the
owning central runner must also accept F/E/C and final generation/inventories.
Nothing has been executed merely by integrating these sources. Central still
owns canonical fixture packaging, binary compilation, actual stack/browser
execution, final schema/client inventories and overall acceptance.

## Cleanup and bounded evidence

All child commands are foreground process groups with deadlines and termination
followed by wait. The browser closes its fresh context and owned browser, including
a SIGTERM handler. The probe tracks actual approval/execution IDs and recovers
uncertain HTTP/browser mutations only under its exclusive run-owned pack/actions.
It revokes its approvals, cancels and polls unfinished executions to terminal
state, pauses only its two owned queues, cancels bounded exact-ID queued items,
awaits item settlement, and unlinks only its own CLI/MCP/inspector credential
files. Cleanup errors fail the
run. It does not delete fixture identities, keys, packs or rows behind active
writers. The lifecycle runner must still stop and join all services, verify labels
and remove its exact owned volumes. Check browser shutdown on failed/timed-out
runs before deleting any owned process/profile directory.

`g-interoperability/evidence.json` is exclusive mode-0600 bounded output containing
case names, IDs, selections, counts and hashes. Never raw Key/config values,
credentials, request expressions or subprocess output. Provisioned credentials
are private operational inputs, not publication artifacts. On provisioning failure,
the provisioner freezes only newly created identities and preserves their ID ledger.

## Central validation commands

After integration and the current freeze, use the central build/test scheduler:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/acceptance -p test_interoperability_contract.py
cargo test -p attune-cli --bin attune-mcp interoperability::tests -- --test-threads=4
cargo test -p attune-cli --bin attune-mcp interoperability_diagnostic_tests -- --test-threads=4
cargo test -p attune-cli commands::permission -- --test-threads=4
cargo test -p attune-common interoperability_inspection::tests -- --test-threads=4
cargo build -p attune-common --example interoperability_inspector --locked -j 2
cargo check --all-targets --workspace
```

Compile `InteroperabilityJavaSdk.java` centrally with the matching SDK jars and
`javac -cp <actual-sdk-and-jackson-classpath> -d <owned-probe-classes> <source>`.
Use the already installed browser module and executable. Source parsing and
offline unit tests do not count as actual-server/browser acceptance. Central must
run the two foreground probe commands above in the owned lifecycle and retain
the exact source/client/image/backend inventories and joined teardown results.
