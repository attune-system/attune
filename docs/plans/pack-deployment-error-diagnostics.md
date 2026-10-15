# Pack deployment error diagnostics: corrections to make

Date: 2026-10-13

Status: implemented in the working tree; targeted validation passed. The full deployment failure matrix remains outstanding.

The findings below describe the 2026-10-13 inspection snapshot, including pre-existing uncommitted changes. Their line numbers are historical references. Implementation follows upload, local registration, remote installation, candidate tests, component loading, activation, batch deployment helpers, and CLI/web presentation.

## Implementation

- Shared `PackDiagnostic` records now include authored metadata pointers, YAML positions, component identity, hints, and schema reference chains. `DeploymentDetails` carries stage, deployment outcome, pack/version, and install/release identity. The existing diagnostic file key remains `path`.
- Metadata validation runs before candidate code or publication. Manifest normalization is shared, malformed YAML has a consistent client-error response, present invalid values cannot silently become defaults, and component/runtime/reference failures block activation instead of disappearing into logs.
- API error context preserves permission, conflict, retryable database, validation, and infrastructure categories. Nested validation failures no longer print submitted scalar values or Rust validation structs. Archive diagnostics identify members and limits; Git failures no longer copy stderr into public responses.
- Normal activation errors are persisted with their original diagnostics. Post-commit publication failures explicitly report committed activation and required storage reconciliation. Responses use the pack snapshot assembled inside the activation transaction rather than an ambiguous follow-up lookup.
- Batch helpers account for every item, retain prior successes, return structured per-item failures/warnings, and report actual component counts. The environment endpoint explicitly reports detection-only operation and zero builds; it does not claim to build worker environments.
- CLI HTTP failures retain diagnostic details and render readable locations or JSON/YAML error envelopes. Failed/rolled-back installs and timeouts cannot fall through to a success banner. Candidate test overrides preserve test results while recording successful deployment with warnings. Deployment requests have a wait budget matching the server's test/setup budget.
- The web UI uses a validated shared error decoder, preserves readable multi-line diagnostics and warnings, checks install state, polls through activation, and reports polling failures. It no longer redirects away before the outcome can be reviewed. Safe generated-client fallback messages are preserved by the API-generation script.

Apply `migrations/20261013000015_pack_deployment_diagnostics.sql` before starting the updated API/services. It adds the nullable `pack_install.diagnostic_details` column. It does not rewrite existing migrations or reset data.

Pure parser/formatter, request-rejection, CLI, and web regressions were added. API/database regressions cover syntax locations, invalid action fields, transaction rollback, and preservation of the actual activation failure. Final command outcomes are recorded below when validation completes. Live-worker setup failures, proxy disconnects during long deployments, and post-commit filesystem failure injection still need the full deployment failure matrix; source changes alone are not proof of those runtime scenarios.

### Validation results

- `cargo check --all-targets --workspace` passed without Rust warnings.
- `cargo sqlx prepare --workspace -- --all-targets --workspace` passed against an owned disposable PostgreSQL 18 container, which was removed afterward. SQLx reported no query macros to prepare; repository queries are dynamic.
- Targeted shared diagnostics tests passed, 6 tests. The initial metadata checker suite passed, 15 tests.
- Scoped API model regeneration and web type-checking passed.
- Installation-page tests passed, 12 tests. Combined web runs also encountered worker-startup timeouts before some files could run, so those runs are not counted as successful full-suite runs.
- The owned API/database rerun passed all 3 cases: syntax locations, authored field diagnostics, and preservation of the actual activation failure in install history. The initial fixture ownership conflict was corrected using the existing install-only helper. The runner removed its owned containers, volumes, and network.
- All nine targeted Rust suites passed: shared diagnostics, pack registry, workflow parser, API errors, API request rejections, CLI client, CLI pack commands, metadata checker, and API pack helpers. The corrected metadata checker rerun passed 16 tests; the API pack helper rerun passed 37 tests. Archive assertions now check readable link descriptions and member names instead of Rust enum names. The final `cargo check --all-targets --workspace` passed without warnings.

## Findings

Deployment errors do not yet consistently tell the operator which file and field to change. Some errors already include a filename or component ref, but that context is frequently lost between parsing, preflight, repository calls, and HTTP responses. Other failures only reach service logs, despite a successful deployment response.

The CLI's HTTP error formatter already extracts the API's message, status, code, and request ID instead of dumping a Rust response struct. Keep that behavior. There are still internal representations in user-facing messages: `SchemaError` formats its Rust enum with `Debug`, archive rejection formats a TAR entry type with `Debug`, and several API errors embed serialized JSON arrays inside an error string.

## Required diagnostic contract

Use one structured deployment diagnostic through common code, the API, persisted install status, CLI output, and web display. Extend the existing `PackDiagnostic` rather than building a separate string-only format for every stage. Its current definition in `crates/common/src/pack_check.rs:43-50` has severity, code, file path, and message, but no metadata path or deployment context.

Each diagnostic should carry:

- Stable code and severity.
- Deployment stage, such as archive extraction, metadata validation, permission admission, candidate distribution, test execution, component loading, configuration binding, or projection publication.
- Pack ref and version when known, component kind and ref when known.
- Pack-relative source file and an escaped JSON Pointer into the authored YAML document. For example, `actions/send.yaml` and `/parameters/recipient/type`. Keep a readable field notation for human display if useful.
- Line and column when available from YAML parsing. Do not invent a field path for syntax errors that prevent identifying the field.
- A plain-language cause and a specific adjustment or next step.
- Install ID, test execution ID, request ID, and release ID/digest when applicable.
- Deployment outcome separately from the error: candidate rejected with active release unchanged, activation committed but projection unavailable, deployed with warnings, or outcome unknown.

Files and paths must describe the operator's source, not `.staging`, `.pack-test-*`, temporary download directories, or immutable release storage paths. Preserve the actual authored alias when reporting a schema location. A `parameters` field should not be reported as `param_schema` just because that is the internal model name.

Not every failure has a YAML fix. Transport, disk, database, broker, and worker failures should identify the responsible service or configuration setting instead. Configuration values preserved in the database should identify that configuration revision and component, rather than pretending the value came from the uploaded YAML.

Never include secret values, credentials, raw request/response bodies, or unfiltered subprocess output. Verbose mode is not permission to expose secrets. Full internal error chains belong in restricted, redacted diagnostics correlated by ID.

Example proposed API response:

```json
{
  "error": "Pack 'example' was not deployed: action metadata is invalid.",
  "code": "PACK_METADATA_INVALID",
  "details": {
    "stage": "metadata_validation",
    "outcome": "active_release_unchanged",
    "diagnostics": [
      {
        "severity": "error",
        "code": "action.timeout_invalid",
        "path": "actions/send.yaml",
        "metadata_path": "/timeout_seconds",
        "component_ref": "example.send",
        "message": "Timeout must be a positive integer in seconds.",
        "hint": "Set timeout_seconds to an integer greater than zero."
      }
    ]
  }
}
```

Normal CLI display should say:

```text
Pack 'example' was not deployed. The active release is unchanged.
actions/send.yaml at /timeout_seconds: Timeout must be a positive integer in seconds.
Set timeout_seconds to an integer greater than zero.
```

JSON/YAML CLI modes should return the same structured diagnostics, not this human string wrapped in another error string. Debug/verbose output can add redacted implementation details without replacing the useful normal-mode message.

## Priority 1: missing or misleading failures

### 1. Stop silently activating invalid or incomplete components

Evidence:

- `crates/common/src/pack_registry/loader.rs:1477-1486` catches an action-linked workflow loading failure and continues creating the action without its workflow link.
- Dashboard parsing/required-field failures become skip warnings at `loader.rs:1807-1826`. Other component loaders also accumulate warnings and skip components.
- `crates/api/src/routes/packs.rs:2466-2493` rejects skipped rules, but otherwise logs loader warnings and keeps only `load_result.projections`. `PackInstallResponse` in `crates/api/src/dto/pack.rs:293-316` has no component diagnostics field.

Correction: make invalid required metadata and unresolved workflow links blocking deployment errors. Classify genuinely non-blocking omissions separately and return them in successful responses and install status. Apply the same policy across component kinds. Do not let `force` suppress structural or authorization errors; reserve any override for an explicitly identified test policy.

Example: `actions/deploy.yaml at /workflow_file: Workflow actions/workflows/deploy.workflow.yaml could not be loaded; no release was activated.` Include the companion graph's diagnostic separately.

Acceptance: a workflow action referencing an unknown task action cannot activate unlinked; an invalid dashboard cannot disappear without an explicit diagnostic. Non-blocking warnings appear in normal CLI and web output.

### 2. Preserve failure category through component loading

Evidence: `crates/api/src/routes/packs.rs:2495-2503` preserves `PermissionDenied`, but converts every other loader failure into `BadRequest`. A database or filesystem failure therefore looks like invalid user metadata, bypassing the shared SQLx mapping, including retryable database errors.

Correction: attach pack/stage/source context without replacing the underlying category. Use 400 for malformed input, 422 for semantically invalid metadata, 403 for permission failures, 409 for conflicts, and appropriate 5xx responses for service failures. Retain the existing retryable-database contract.

Acceptance: equivalent loader failures produce distinct validation, permission, conflict, and infrastructure codes/statuses, with no SQL details in the public body.

### 3. Report the real activation error in install history

Evidence: `PackActivationFailureGuard::drop` at `crates/api/src/routes/packs.rs:135-153` stores only `Pack activation was interrupted`. The request may return a specific component error while the persisted attempt loses that cause.

Correction: explicitly finalize failed activation attempts with the same structured diagnostic returned by the API. Keep the drop guard as a fallback for cancellation or unexpected interruption, not the normal error-reporting path. Await normal failure persistence and preserve the primary failure if recording it also fails.

Acceptance: querying an install after rejected metadata returns the same file, field, code, and stage as the original HTTP failure.

### 4. Distinguish committed activation from post-commit publication failure

Evidence: `crates/api/src/routes/packs.rs:2542-2564` marks the install finished and commits the database transaction before publishing/finalizing the active filesystem projection. A later HTTP 500 can coexist with a succeeded install record. The first publication message does acknowledge the committed database state, but the finalization message does not explain the outcome or recovery.

Correction: record and expose projection publication/finalization failures after commit. Specify whether the database release is active, whether its filesystem projection is available, and what reconciliation is needed. Do not claim rollback or recommend a blind retry after commit. Keep immutable release activation and fail-closed storage semantics intact.

Acceptance: injected publication and finalization failures yield consistent HTTP, install-history, CLI, and web outcomes, with a release/install ID and recovery instruction.

### 5. Prevent batch helpers from dropping failures and losing counts

Evidence:

- `download_packs`, `crates/api/src/routes/packs.rs:4810-4835`, omits a successful download from both results lists when subsequent manifest reading/parsing fails. Missing ref/version become `unknown` and `0.0.0` instead of errors. Source detection at line 4795 uses `?`, aborting the whole batch rather than recording an item failure.
- `register_packs_batch`, `packs.rs:5252-5299`, silently omits an activated pack if the follow-up pack lookup fails or returns none. Test-result conversion can also abort the entire batch after earlier packs have committed.
- Failed registrations at `packs.rs:5301-5307` use `pack_ref: unknown` and `error_stage: registration` regardless of what is known.

Correction: return exactly one outcome per requested item, preserve the real stage and known pack identity, and distinguish deployment success from post-deployment response/history failure. Include structured diagnostics per item. Document partial success and make clients inspect item outcomes, not just HTTP 200.

Acceptance: mixed-validity batches account for every input, retain prior successes, and never report an activated pack as simply absent from the results.

## Priority 1: source locations and plain-language causes

### 6. Add source context once at every component boundary

Evidence: `read_yaml_files` at `crates/common/src/pack_registry/loader.rs:3752-3760` returns only a basename and contents. Per-field helpers propagate errors without the filename, such as action schema/visibility/retention/timeout parsing at `loader.rs:1516-1523,1582-1597`. Many repository calls likewise propagate with bare `?`.

Correction: retain a source descriptor with pack-relative path and component identity. Attach it to errors around each parse, validator, reference lookup, and persistence operation. Keep field/index pointers in validators; avoid reconstructing locations from message strings. Translate known constraint failures into component/field errors at the owning boundary rather than exposing constraint names.

Example: `actions/send.yaml at /timeout_seconds: Expected an integer greater than zero`, not just `timeout_seconds must be greater than zero`.

Also fix mislabeled fields: `parse_log_retention_policy` and `parse_log_retention_limit` at `loader.rs:3780-3815` hardcode `log_retention_*` in errors even when called for `artifact_retention_*` at lines 1593-1596. Pass the actual field identity.

Acceptance: invalid action, sensor, trigger, runtime, queue, policy, dashboard, cache, and permission-set fields each identify their directory, file, and authored metadata field.

### 7. Replace schema enum names with useful messages without exposing values

Evidence: `crates/common/src/value_schema/mod.rs:195-204` defines `SchemaError` with `#[error("{code:?} at {path}")]`. A normal user can receive `InvalidDefinition at /...`, `StructuralOverride at /...`, or `MissingReference at /...`. Formatting also drops the structured `source_ref` and `reference_chain`.

Correction: map schema codes to stable wire codes and human explanations. Preserve pointers and reference chains as structured fields, prefix schema-local pointers with the authored field path, and attach the source file. Keep the current no-values/no-validator-display safety rule. Explain the allowed adjustment, such as declaring a dependency, defining a missing type, or removing a forbidden structural override.

Acceptance: normal responses contain no Rust enum spelling; malformed schemas identify the full authored path and give a useful reason without including defaults or secret values.

### 8. Preserve named-type file and consumer provenance

Evidence: `crates/common/src/pack_registry/schema_types.rs:35-76` drops file paths and underlying read/parse context in several errors, including `Duplicate named type definition`. Preflight missing/policy-denied references at lines 178-244 lack the consumer's file/field. `read_consumer_schemas` at lines 396-420 reports `Invalid pack schema source YAML` without filename or YAML location, and records `fields[0]` rather than the actual authored alias.

Correction: retain file, authored schema field, and reference-use pointer for definitions and consumers. Show both files for duplicate definitions and both ends of an invalid reference when authorized. Preserve syntax line/column. Identify limit name and configured threshold for source-budget failures rather than only saying the budget was exceeded.

Acceptance: an invalid type in an otherwise unused file still identifies that file; a missing cross-pack type points to the exact consuming `type` field and the relevant `pack.yaml` dependency entry.

### 9. Keep permission-use locations through admission

Evidence: `crates/common/src/pack_registry/permissions.rs:35-36,53,82,121,157` discards filenames while reading. `PermissionUse` at lines 25-31 holds no source information. Errors such as `Pack permission definitions exceed the registering identity's authority` at lines 216-221 and `Pack references unavailable execution permission sets` at lines 258-263 cannot identify the offending declaration or use.

Correction: retain source descriptors for definitions, grant indexes, action defaults, rule/queue refs, sensor config refs, and workflow task refs. Report the requested ref and failing grant resource/action where disclosure is allowed. Distinguish bad metadata from insufficient registering-identity authority. Do not reveal whether an inaccessible external resource exists.

Example: `permission_sets/deploy.yaml at /grants/0/actions: The registering identity cannot delegate the requested action. Reduce this grant or deploy with an authorized identity.`

Acceptance: an unavailable permission ref and an excessive grant identify the submitted source location while preserving non-disclosure rules.

### 10. Locate reference errors at the use, including workflow task indexes

Evidence:

- Unknown rule trigger/action warnings at `crates/common/src/pack_registry/loader.rs:2487-2493,2511-2517` include refs but not filenames or field paths.
- Workflow action refs are collected into a ref list at `loader.rs:2774-2791`, losing the individual task/index location.
- Rule secret-template errors at `loader.rs:2563-2573` do not distinguish the offending `action_params` use from `trigger_params`, or identify the file.
- `crates/api/src/routes/packs.rs:2468-2472,2513-2517` embeds warnings/admission-failure arrays as JSON inside the error string.

Correction: keep each reference use with its file and pointer, even if database lookups deduplicate targets. Put warning/admission arrays in `details.diagnostics`, not inside `error`. For workflows, attach the graph path such as `/tasks/2/action` and retain the action metadata's `/workflow_file` as a related location. Translate managed-sensor admission failures into the relevant rule placement/config and sensor schema locations.

Acceptance: two tasks referencing the same missing action identify both use sites. CLI/web display a readable diagnostic list rather than escaped JSON.

### 11. Turn log-only runtime and sensor-reference failures into diagnostics

Evidence: runtime resolution at `crates/common/src/pack_registry/loader.rs:3362-3366` logs a warning and returns an unknown runtime sentinel, without adding a `PackLoadResult` diagnostic. Sensor trigger resolution at lines 3405-3409 conflates repository errors and missing triggers, logs a warning, and returns an unresolved ref. Linkage skips at lines 3425-3445 also only log.

Correction: reject missing required runtimes or trigger refs with source file and `/runner_type` or `/trigger_types/<index>`. Keep repository failures distinct from missing metadata references. Explain when runtime definition installation is needed versus worker runtime availability. Return intentional ownership-related linkage skips as explicit warnings if they are valid behavior.

Acceptance: unknown runner types do not silently produce runnable actions with no runtime; a database failure cannot masquerade as an unknown sensor trigger.

### 12. Use one validated manifest interpretation across deployment routes

Evidence: registration parses `pack.yaml` into a generic YAML value at `crates/api/src/routes/packs.rs:1973-2052`. Wrong-typed ref/version are reported as missing, malformed sequences lose entries through `filter_map`, and present invalid values can fall back to defaults. Registration reads legacy `name`, `config_schema`, `metadata`, and `keywords` here, rather than canonical `label`, `conf_schema`, `meta`, and `tags`. `validate_pack_environment` at `crates/common/src/pack_check.rs:127-148` only enforces environment and limit diagnostics, not the full check report.

Correction: normalize and validate the manifest once before tests and publication, using the same canonical rules as local checking. Distinguish absent, null, wrong-typed, unsupported, and conflicting-alias values. Reject present invalid values instead of discarding them. Share parsing across upload/register/install/test preflight so they return the same diagnostic.

Fix the status mismatch too: upload syntax failures are `BadRequest` at `packs.rs:1712-1713`, but registration at line 1977 and remote install at lines 3660-3662 and 3731-3733 call the same problem `InternalServerError`.

Acceptance: the same malformed manifest produces the same 400/422 diagnostic on every route. Invalid `/dependencies/1` is not silently dropped, and canonical schema metadata is not ignored.

## Priority 2: tests, infrastructure, and presentation

### 13. Expose the failed test runner and its configuration path

Evidence: test configuration parsing at `crates/api/src/routes/packs.rs:1321-1339` loses nested YAML field context. `TestConfig::validate_policy` at `crates/common/src/test_executor.rs:50-56` iterates runner values, so invalid timeout errors cannot name the runner. Worker failure summaries at `crates/worker/src/service.rs:2502-2513` store only pass counts in `error_message`, although suite/case details are retained in `test_result`. API rejection at `packs.rs:2170-2175,3859-3864` returns the summary but not the install ID or detailed test result.

Correction: retain runner names and paths such as `/testing/runners/python/timeout` and `/testing/runners/python/entry_point`. Return the failed suite, cause, install ID, test-result lookup information, and safe log/artifact references. Distinguish test assertions, unsupported runner/result formats, process startup, timeout, dependency setup, and candidate transport failures. Show setup failures even when no test case ran. Return an explicit warning when tests are forced past failure or skipped due to absent/disabled metadata.

Acceptance: a missing runner entrypoint or invalid runner timeout identifies `pack.yaml` and the runner key; a failing candidate test can be inspected even for a new pack that was never installed.

### 14. Align dependency diagnostics with the environment being checked

Evidence: `crates/api/src/routes/packs.rs:3666-3686` reads legacy top-level Python/Node requirements and treats dependency strings as refs with wildcard versions. `crates/common/src/pack_registry/dependency.rs:238-265` detects runtimes on the calling machine, which for this API path is the API service, not a selected worker. The resulting message can simply say `Runtime 'python3' not found on system`.

Correction: identify the exact authored dependency entry and required/detected version, and state which service/worker was checked. Align dependency parsing with the canonical pack dependency format, including version constraints and declared `runtime_deps`. Use worker capability/runtime checks for execution requirements rather than directing users to install an interpreter in the API container. Keep invalid version syntax as a metadata error, separate from detection failure.

Acceptance: missing worker Python and malformed pack dependency constraints have different codes and actionable locations. Upload/register/install agree on the dependency checks they promise to enforce.

### 15. Name archive entries and extraction limits, without Rust type dumps

Evidence: `crates/common/src/pack_registry/archive.rs:74-81` rejects a forbidden TAR entry before reading its path and formats `kind` with `Debug`. ZIP rejection at lines 135-139 also omits the entry name. Size/count checks at lines 72,107,131,159 do not receive entry identity. Upload extraction at `crates/api/src/routes/packs.rs:1695` wraps every extraction error as a bad request, including destination I/O failures.

Correction: identify the archive member or entry index, describe the rejected type in plain language, and return actual versus allowed size/count when known. Separate unsafe/corrupt input from unavailable server storage. Use an archive-member location, not a fabricated YAML field path. Bound and escape untrusted filenames before displaying them.

Acceptance: a forbidden symlink says which member to remove; a quota error names the relevant limit; disk-full extraction remains a server/storage error.

### 16. Add stage and recovery context to infrastructure failures, with redaction

Evidence: object-key/body/put/head failures at `crates/api/src/routes/packs.rs:2271-2297` often return only `error.to_string()`. Storage/publication errors at lines 2070-2075 and 2240-2244 have a stage description but lack pack/version/install context and next steps. Git clone/checkout errors at `crates/common/src/pack_registry/installer.rs:280-308` include unfiltered stderr. Shared middleware at `crates/api/src/middleware/error.rs:165-215` can expose database error text or constraint names.

Correction: return safe stage-specific messages, responsible service/configuration, retry guidance, and correlation IDs. Keep implementation/provider details in restricted redacted logs. Scrub Git output and remote URLs, including credentials, query strings, and tokens, before any user-facing or verbose display. Translate known constraint failures into domain errors; do not blanket-redact useful metadata causes into `Internal server error`.

Example: `Pack 'example' 1.2.3 could not publish its release archive to object storage. No release was activated. Ask the operator to check the API object-storage configuration. Install 42.`

Acceptance: transport/storage/DB/broker failures retain stage and outcome, never expose credentials or SQL details, and are distinguishable from changes needed in pack YAML.

### 17. Carry structured diagnostics through HTTP and CLI output

Evidence: `ErrorResponse` supports `details` at `crates/api/src/middleware/error.rs:14-22`, but `ApiError::into_response` at lines 149-154 never supplies them. The CLI's `ApiError` at `crates/cli/src/client.rs:35-43` stores `_details` without a serde rename for the API's `details` field, so the actual field is not captured. `api_error_message` at lines 107-128 only retains a formatted message/status/code/request ID. Final CLI failure output at `crates/cli/src/main.rs:307-309` is always a plain stderr string, regardless of JSON/YAML selection.

Correction: allow API errors to carry structured diagnostics and preserve them in a typed CLI request error. Render a concise diagnostic list in normal mode and a documented error envelope in machine-readable modes. Ensure parser/multipart/body-limit/auth rejections use a compatible error format too; do not assume every error comes from `ApiError::into_response`. Add a bounded, safe fallback for non-JSON proxy responses without dumping the body.

Acceptance: file and metadata pointer survive API to CLI unchanged; JSON/YAML failure output parses and exits nonzero; normal mode contains neither Rust structs nor escaped response JSON. Keep the existing path-aware success-response parser in `client.rs:76-92` for API contract mismatches, but identify those as client/server contract errors, not invalid pack metadata.

### 18. Make terminal failure and timeout handling truthful in CLI commands

Evidence: `poll_pack_install` at `crates/cli/src/commands/pack.rs:1114-1118` prints a timeout and returns `Ok(None)`. Failed/rolled-back terminal statuses at lines 1137-1166 also return `Ok`. Install/upload/register callers then print success messages and return `Ok` at lines 1053-1086,1322-1347,1462-1495. Callers copy status/test result but do not include the polled `error_message` in their final response. The rolled-back branch at lines 1142-1144 omits that message even in table mode. `succeeded` is described as tests passed, though tests may have been skipped.

Correction: distinguish deployment outcome from test outcome. A non-forced terminal deployment failure must exit nonzero in every output mode. A polling timeout must report outcome unknown and provide the install-status lookup, not fall through to installed successfully. An activated release with explicitly overridden test failure needs a warning and both outcomes, not a false deployment failure. Align HTTP/client waiting budgets with the API's potentially hour-plus candidate-test wait at `packs.rs:1514-1520`; the default client timeout is 300 seconds at `client.rs:188`.

Acceptance: status failure, rollback, network timeout, and polling timeout cannot produce an unconditional success banner. JSON/YAML retain the failure cause and IDs. An ambiguous HTTP timeout advises checking the attempt before retrying.

### 19. Render full diagnostics in the web UI and respect install state

Evidence: `web/src/pages/packs/PackInstallPage.tsx:61-74` extracts the API message, which is useful, but ignores `details`, codes, and related locations. Lines 97-111 report installation success and redirect based on HTTP success without checking `install_status`. `web/src/hooks/usePackTests.ts:92-95` polls only pending/running and stops at activating. Generated `web/src/api/core/request.ts:292-311` uses generic status messages or a raw JSON body in its fallback error message.

Correction: use a shared, validated error decoder and diagnostic renderer. Show a persistent readable list with file, field, cause, adjustment, stage, and IDs. Preserve multi-line messages and long file paths. Do not redirect away from failures or warnings before the user can read them. Check terminal install state, continue polling through activating, and display failed/rolled-back causes and polling errors. Avoid using generic generated `Error.message` as the final user explanation when a structured API body exists. Update the generator/custom client layer rather than hand-editing generated files alone.

Acceptance: validation, 403, 409, 422, 5xx, and non-JSON proxy errors display useful safe text. Pending/activating is not labelled installed; failed installs stay visible; warnings remain accessible after deployment.

### 20. Do not report environment builds that never happened

Evidence: `build_pack_envs` at `crates/api/src/routes/packs.rs:5096-5111,5139-5154` only logs that environments would be built, but increments built counters and returns build-shaped results. Missing executables are reported as unavailable on an unspecified system at lines 5118 and 5161. `pack.yaml` ref defaults to `unknown` at lines 5074-5078.

Correction: implement worker-owned setup with real failure reporting, or explicitly return unsupported/detection-only status instead of build success. Identify runtime declaration locations, dependency files such as `requirements.txt` or `package.json`, selected worker, setup command stage, and safe log references. Keep runtime environments outside packs as required by the repository contract.

Acceptance: successful build results require actual setup completion; detection-only responses cannot be mistaken for deployed runtime readiness.

## Implementation order and validation gate

1. Define the shared structured diagnostic and API/CLI/web rendering contract. Add source descriptors before converting more errors into strings.
2. Fix lost failures and incorrect outcome/status handling, especially component skip warnings, persisted activation causes, post-commit publication, and batch accounting.
3. Add metadata pointers and plain-language schema messages across manifest, component, type, permission, workflow, reference, and test validators.
4. Cover infrastructure and archive errors, then update generated API contracts and all client displays together.

Add regression cases for every correction above. Use pure parser/formatter tests where possible, disposable repository/API fixtures for status and persistence, and owned storage/worker failures for activation outcomes. Snapshot normal, verbose, JSON, and YAML CLI failures and web error panels. Include synthetic secret canaries and assert that they never appear in API bodies, normal/verbose output, persisted public diagnostics, or logs intended for users.

The release gate should require that every user-correctable metadata failure has an exact file and field location whenever those are knowable. Every service failure must identify its stage and next step. Every returned outcome must agree with what committed, and no blocking failure may exist only in service logs.
