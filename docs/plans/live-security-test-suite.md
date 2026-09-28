# Live security test suite implementation plan

**Status:** Proposed implementation plan  
**Scope:** Black-box security tests against an already-running Attune instance  
**Out of scope:** Rust unit and integration tests, the Python E2E suite, service startup, host sandboxing, and resource-exhaustion tests

## Decision

Implement the suite in Python 3.12 with pytest. Create a standalone project at `live-security-tests/` with its own dependency lock, pytest configuration, fixtures, probe pack, commands, reports, and CI workflow. The suite must not import code from `tests/`, invoke Cargo, start Attune services, or join the E2E Docker Compose lifecycle.

Use one direct command for local and CI runs:

```bash
cd live-security-tests
uv sync --frozen
uv run pytest -m smoke
```

Profile markers select `contract`, `smoke`, `core`, `lifecycle`, or `token_replay`. Pytest options may narrow a profile, but no second test runner or wrapper may select a different test inventory.

The suite should test the security boundaries that Attune promises:

- Access-token and execution-token authentication.
- Identity, permission-set, owner-scope, and tenant authorization.
- Execution-token authority derived from `permission_set_refs`.
- Execution timeout and cancellation behavior.
- Protection of trusted execution context from caller-supplied parameters.
- Absence of credentials and secret values from platform-owned API responses, audit details, and error messages.

Attune intentionally lets trusted actions run arbitrary code. The suite must not treat filesystem, subprocess, CPU, memory, or outbound-network access as denied capabilities. It must not probe those capabilities on a shared live worker.

## 1. Suite boundary

Use this repository layout:

```text
live-security-tests/
├── README.md
├── pyproject.toml
├── uv.lock
├── pytest.ini
├── config.example.toml
├── contracts/
│   ├── api-operations.yaml
│   ├── authorization-matrix.yaml
│   └── target-capabilities.yaml
├── probe-pack/
│   ├── pack.yaml
│   └── actions/
│       ├── execution_context_probe.yaml
│       ├── execution_context_probe.py
│       ├── execution_token_probe.yaml
│       ├── execution_token_probe.py
│       ├── lifecycle_probe.yaml
│       └── lifecycle_probe.py
├── src/attune_live_security/
│   ├── client.py
│   ├── config.py
│   ├── evidence.py
│   ├── matrix.py
│   ├── ownership.py
│   ├── polling.py
│   ├── safety.py
│   └── target.py
└── tests/
    ├── conftest.py
    ├── authentication/
    ├── authorization/
    ├── execution_tokens/
    ├── lifecycle/
    ├── secrets/
    └── contracts/
```

The separation rules are strict:

- `live-security-tests/pytest.ini` sets `testpaths = tests` relative to that project.
- The project requires Python 3.12 and has its own virtual environment and pinned `uv.lock` file.
- Pytest is the only test runner. The project does not use the repository's E2E pytest configuration.
- No module under `live-security-tests/` imports `attune/tests`, `tests/helpers`, or Rust test utilities.
- The suite communicates with Attune only through published HTTP and WebSocket interfaces. It does not connect to PostgreSQL or RabbitMQ.
- The runner accepts an existing target URL. It never starts, stops, restarts, migrates, or tears down the Attune deployment.
- The probe pack belongs only to this suite. The E2E fixture packs must not reference it.
- Add a separate `.github/workflows/live-security-tests.yml`. Do not add the live suite to the Rust `Tests` job or the E2E smoke job in `.github/workflows/ci.yml`.
- Add dedicated commands such as `make live-security-smoke` only as thin entrypoints. They must not become dependencies of `make test`, `make test-integration`, or `make e2e-test`.

Small overlap in assertions is acceptable. An E2E test may still confirm that a normal user journey works, while the live security suite tests a complete denial matrix. Do not migrate all tests marked `security` from `tests/e2e/tier3` mechanically.

## 2. Threat model and test oracle

Treat these actors separately:

| Actor | Trust | Security question |
|---|---|---|
| Unauthenticated caller | Untrusted | Can it reach a protected operation or obtain useful sensitive data? |
| Authenticated user | Partially trusted | Can it act outside its identity, role, tenant, or owner scope? |
| Execution caller | Untrusted input source | Can supplied parameters change the selected action, execution identity, permission sets, or trusted environment? |
| Installed action code | Trusted arbitrary code | Does its execution token grant exactly the configured Attune API authority? |
| Suite operator | Trusted | Can the harness prove a result without exposing credentials or damaging unrelated resources? |

An expected denial must assert the documented status and error class. Do not accept any `4xx` response as equivalent. A `404` used to conceal an inaccessible object is distinct from a validation `400`, an unauthenticated `401`, and an unauthorized `403`.

The suite should generate most authorization cases from `contracts/authorization-matrix.yaml`. Each row identifies:

- The subject fixture and token type.
- The HTTP operation and resource fixture.
- The expected allow or deny decision.
- The expected status or status set.
- Whether the operation mutates state.
- The cleanup owner.
- The evidence fields safe to retain.

Keep a checked-in `api-operations.yaml` inventory derived from the supported Attune OpenAPI document. A contract test compares the target's operations with that inventory and fails with an "unclassified operation" report when a new protected operation lacks a matrix decision. Version the inventory by Attune API version, not by test run.

## 3. Target and safety contract

The runner requires explicit configuration. It must not infer credentials from the developer's Attune CLI profile.

```text
ATTUNE_SECURITY_BASE_URL
ATTUNE_SECURITY_ADMIN_TOKEN
ATTUNE_SECURITY_TARGET_ID
ATTUNE_SECURITY_EXPECTED_TARGET_ID
ATTUNE_SECURITY_RUN_PROFILE
ATTUNE_SECURITY_ALLOW_MUTATION
ATTUNE_SECURITY_RUN_ID
```

`ATTUNE_SECURITY_TARGET_ID` comes from a stable, non-secret target metadata endpoint or an operator-managed deployment value. The runner compares it with `ATTUNE_SECURITY_EXPECTED_TARGET_ID` before creating anything. A hostname match alone is not enough.

The preflight must fail closed unless all applicable checks pass:

1. The base URL uses HTTPS, except for an explicit loopback development target.
2. The target identity matches exactly.
3. The health and API-version checks pass.
4. The admin token can perform the required fixture operations.
5. The deployment contains the dedicated security-test tenant or equivalent isolation scope.
6. Mutation is explicitly enabled for any profile that creates resources.
7. No fixture name from another active run uses this run ID.
8. The requested profile stays within its execution and request budget.

Use a run ID such as `sec-<date>-<random>`. Prefix every identity, permission set, key, pack, action-owned fixture, cache namespace, queue item key, trace tag, and synthetic secret marker with that ID. Register cleanup as soon as each resource is created.

Cleanup may delete only resources recorded in the run's ownership manifest. It must never delete by age, a broad `test-*` prefix, or discovery of "recent" objects. Report normal cleanup failures before any janitor recovery. Keep failed-run manifests as encrypted CI artifacts with a short retention period.

### Profiles

Define these profiles from the start:

| Profile | Intended target | Mutation | Concurrency | Budget |
|---|---|---:|---:|---:|
| `contract` | Any approved target | None | 8 HTTP workers | 60 seconds |
| `smoke` | Dedicated security-test tenant | Small fixtures | 4 HTTP workers, 1 action | 2 minutes |
| `core` | Disposable or dedicated test instance | Full owned fixtures | 8 HTTP workers, 2 actions | 10 minutes |
| `lifecycle` | Disposable or dedicated test instance | Executions and cancellation | Serial | 5 minutes |
| `token-replay` | Disposable instance only | In-memory token relay | Serial | 5 minutes |

The `token-replay` profile is opt-in because proving post-completion token rejection requires the harness to hold an execution token briefly. The probe sends the token over TLS to an ephemeral suite callback. The callback keeps it only in memory, suppresses request logging, never writes it to a report, and clears it after the assertion. The runner must refuse this profile unless the target declares itself disposable.

## 4. Bounded probe pack

Keep the probe pack small and review it as security-sensitive test code. Use Python 3.12 and the normal stdin JSON parameter contract. Do not use a shell entrypoint.

Every probe action enforces these internal limits before doing work:

```text
maximum wall time: 10 seconds
maximum Attune API requests: 40
maximum request body: 16 KiB
maximum response body retained: 4 KiB
maximum stdout and stderr combined: 64 KiB
allowed origin: exact ATTUNE_API_URL origin
allowed paths: action-local enum, not caller-supplied URLs
child processes: none
filesystem writes: none outside the action result mechanism
```

The pack contains three actions:

### `execution_token_probe`

The action receives a list of checked-in case IDs, not arbitrary methods or URLs. Each case maps to a fixed method and path template in the action source. The action substitutes only run-owned resource IDs supplied by the harness. It returns the case ID, status, response content type, and a bounded digest. It never returns request headers, the token, or an unfiltered response body.

Install variants with these permission configurations:

- No `permission_set_refs`. Assert that `ATTUNE_API_TOKEN` is absent.
- `standard`. Assert only documented action, pack, and containing-workflow key and artifact access.
- One named read-only permission set.
- One named mutation permission set limited to run-owned fixture types.
- Multiple named permission sets where union behavior is part of the documented contract.

### `execution_context_probe`

The action reports non-secret execution context fields and booleans such as `token_present`. Tests submit parameters named like `ATTUNE_API_TOKEN`, `ATTUNE_EXEC_ID`, `ATTUNE_ACTION`, `permission_set_refs`, and `identity_id`. The result must show that stdin parameters did not replace trusted environment values or the execution's permission snapshot.

Do not ask this action to print its token. Action stdout is controlled by trusted action code, so deliberate token output is not a platform redaction boundary.

### `lifecycle_probe`

The action sleeps in short bounded intervals and emits a sequence number. It has no child process and allocates no growing buffer. Tests use it to verify timeout, explicit cancellation, terminal state, bounded completion time, and the absence of later API side effects.

The harness applies an outer deadline at least five seconds longer than the declared action timeout. It allows only one lifecycle execution at a time. If the outer deadline fires, the harness requests cancellation once, records diagnostics, and stops the profile. It does not launch more actions after a lifecycle failure.

## 5. Initial test inventory

Implement the first release in the following order.

### Authentication

- Reject a missing bearer token on every protected operation class.
- Reject malformed schemes, malformed JWTs, random opaque tokens, and expired access tokens.
- Reject access tokens on execution-token-only routes.
- Reject execution tokens on user and administrative routes.
- Reject tokens issued for another configured audience or issuer when the deployment enables those checks.
- Verify WebSocket authentication at connection time and reject query-string tokens.

Do not make brute-force or sustained rate-limit tests part of the default live suite. A separate operational test can exercise rate limiting against disposable infrastructure.

### User and tenant authorization

- Generate allow and deny cases for administrator, editor, executor, viewer, unrelated identity, and unrelated tenant fixtures.
- Attempt direct access through both numeric IDs and refs.
- Cover packs, actions, executions, artifacts, keys, inquiries, permission sets, caches, queues, rules, and workflows.
- Verify list endpoints filter inaccessible objects rather than returning data that detail endpoints later deny.
- Verify create, read, update, delete, execute, cancel, configure, and response operations where each verb exists.
- Verify an assigned inquiry cannot be read, answered, or cancelled by an unrelated identity.

### Execution-token authorization

- Verify that an empty permission snapshot omits `ATTUNE_API_TOKEN`.
- Verify `standard` access for the executing action and pack.
- Verify denial for sibling actions, unrelated packs, unrelated executions, and unrelated owner scopes.
- Verify containing-workflow access for child executions and denial outside the containing workflow.
- Verify named permission-set allow and deny rows.
- Verify that direct execution, workflow, queue, rule, retry, and resume paths produce the same configured authority.
- Verify that caller parameters cannot select extra permission sets.
- Verify that a child action receives its resolved permission snapshot rather than all parent permissions.

### Owned resource APIs

- Exercise cache namespaces for system, identity, pack, action, and sensor owners.
- Deny reads, chunk uploads, seals, promotions, and aborts outside the token's owner scope.
- Exercise optimistic generation preconditions with two run-owned generations.
- Deny cross-execution artifact reads and writes.
- Deny key reads outside documented action, pack, and containing-workflow scope.
- Confirm that trace tags, refs, external IDs, owner fields, and request bodies do not change the authorization subject.

### Lifecycle

- Cancel a running bounded probe and require the documented terminal state before the deadline.
- Apply a short action timeout and require termination within a measured tolerance.
- Race cancellation against normal completion for a small fixed number of iterations.
- Confirm that retry and resume issue fresh execution context and preserve configured authority.
- Run post-completion token rejection only in the `token-replay` profile.

### Sensitive data

- Use unique synthetic values, never real secrets.
- Verify that key values and token material do not appear in platform-owned execution metadata, audit details, validation errors, list responses, or unrelated identity responses.
- Verify that configuration APIs return documented redacted forms.
- Verify that failed authorization responses do not disclose object contents or secret-dependent differences.
- Do not claim that Attune can hide a secret that trusted action code deliberately writes to its own stdout or artifacts.

### API contract checks

- Require authentication on every operation classified as protected.
- Detect protected operations missing from the authorization matrix.
- Check CORS, cookie flags, cache-control, content-type, and browser security headers on relevant endpoints.
- Check bounded pagination and documented payload-size failures without sending stress-sized payloads.
- Check that unsupported methods and content types fail without stack traces or internal paths.

## 6. Fast and repeatable execution

Use raw `httpx` requests for malformed-auth and denial tests. A generated client tends to normalize the malformed requests that these tests need. A small typed helper may wrap ordinary fixture creation, but it must preserve status, headers, and raw bounded response bytes.

Apply these speed rules:

- Create the shared tenant fixtures once per run. Create mutable resources per worker or per test.
- Use session-scoped HTTP connection pools with strict connect, read, write, and pool timeouts.
- Generate authorization cases from the matrix rather than writing one setup flow per test.
- Run independent HTTP-only denial cases in parallel.
- Serialize lifecycle tests and any test that changes a shared permission set.
- Poll bounded predicates with jittered backoff. Do not use fixed sleeps as readiness proof.
- Reuse the installed immutable probe pack when its content digest matches. Upload a run-owned version only when the target lacks the expected digest.
- Use idempotency keys where the API supports them. Never retry a non-idempotent write automatically.
- Stop the run after a failed target identity check, an expired fixture-admin token, an action outer deadline, or evidence of cross-run resource access.

Set initial performance gates after measuring the first complete implementation. Before measurements exist, use these budgets as limits rather than speed claims:

- `contract`: 60 seconds.
- `smoke`: 2 minutes.
- `core`: 10 minutes.
- `lifecycle`: 5 minutes.
- Normal cleanup: 60 seconds.
- Zero automatic test retries.

Publish per-phase timing for preflight, fixture setup, tests, and cleanup. Report skips as failures unless the selected target capability manifest explicitly marks the feature unavailable.

## 7. Evidence and reporting

Each failed case records:

- Run ID, test ID, target ID, and target API version.
- Subject fixture label and token type, never the token value.
- HTTP method and normalized path.
- Expected and actual status.
- Sanitized response headers and a bounded redacted body.
- Execution ID and trace tag when applicable.
- Last observed execution state for polling failures.
- Cleanup result for resources created by the case.

Redact bearer tokens, cookies, configured secret values, and synthetic secret markers before writing console, JUnit, JSON, or HTML output. Test the redactor with fixed unit tests inside the standalone project. Refuse to publish an artifact if the final secret-marker scan finds a match.

Produce JUnit XML for CI and a compact JSON evidence file for investigation. Do not store complete API traffic captures by default.

## 8. CI and scheduling

Create `.github/workflows/live-security-tests.yml` with no dependency on `.github/workflows/ci.yml` jobs.

Use these triggers:

- Pull requests that change `live-security-tests/**` run offline harness unit tests and schema validation only. They do not need an Attune target.
- Manual dispatch runs a selected profile against an approved environment.
- A scheduled job runs `smoke` against the dedicated security-test tenant.
- A nightly job runs `core` and `lifecycle` against a disposable or dedicated instance.
- The `token-replay` profile remains manual until its callback transport and secret-handling audit pass.

Store target URLs and credentials in environment-scoped CI secrets. Use environment approval for any non-disposable target. Configure one CI concurrency group per target ID so two mutating runs cannot share the same dedicated tenant.

Do not make live target availability a prerequisite for Rust, web, or E2E CI. The live suite reports its own status and ownership cleanup result.

## 9. Implementation phases

### Phase 0: Confirm contracts

1. Record the supported Attune API version and target identity mechanism.
2. Export the OpenAPI operation inventory.
3. Write the first authorization matrix for access and execution tokens.
4. Review expected `401`, `403`, and concealment `404` behavior with API owners.
5. Define the dedicated tenant, test identities, and named permission sets.

**Exit:** Every first-release assertion has a documented expected result. Unknown behavior is not encoded as a permissive status range.

### Phase 1: Build the safe harness

1. Create the standalone Python project and lock dependencies.
2. Implement configuration, target preflight, run IDs, budgets, redaction, evidence, and ownership manifests.
3. Add harness unit tests that use a local mock HTTP server.
4. Prove that the runner refuses a target mismatch, missing mutation consent, an unsafe profile, and unowned cleanup.

**Exit:** No live action execution yet. Harness tests pass without Rust, Docker Compose, the E2E environment, or public internet access.

### Phase 2: Add HTTP security coverage

1. Implement authentication cases.
2. Implement matrix-generated user and tenant authorization cases.
3. Add OpenAPI inventory drift and protected-route checks.
4. Add fixture setup and exact cleanup through public APIs.

**Exit:** `contract` and HTTP-only `smoke` pass repeatedly against a dedicated test instance. Two runs with different run IDs do not alter each other's fixtures.

### Phase 3: Add the probe pack

1. Implement and review the three bounded probe actions.
2. Add probe-pack metadata checks and a content digest.
3. Implement execution-token, trusted-context, and workflow-child cases.
4. Add execution budgets and the stop-on-deadline rule.

**Exit:** The same probe cases pass through direct, workflow, queue, rule, retry, and resume paths without an unbounded action or leaked credential.

### Phase 4: Add lifecycle coverage

1. Implement serial cancellation and timeout tests.
2. Add a small cancellation-completion race sample.
3. Verify terminal states, elapsed-time tolerances, and cleanup.
4. Design and audit the in-memory callback before enabling `token-replay`.

**Exit:** Ten repeated lifecycle runs finish within budget and leave no owned execution running.

### Phase 5: Automate and enforce coverage

1. Add the separate CI workflow and environment approvals.
2. Publish JUnit, sanitized evidence, timings, and cleanup results.
3. Make unclassified protected OpenAPI operations fail the live suite.
4. Document triage ownership and the procedure for updating the matrix.

**Exit:** Scheduled runs are repeatable, failures contain enough safe evidence to reproduce them, and the suite remains independent of Rust and E2E jobs.

## 10. Acceptance gates

The suite is ready for regular use when all of these conditions hold:

- A fresh checkout can install and run harness unit tests from `live-security-tests/` alone.
- The live runner does not execute Cargo, import E2E helpers, or invoke Docker Compose.
- A target mismatch prevents all mutation.
- Every created resource appears in an ownership manifest before the next operation.
- Cleanup touches only exact manifest entries and reports leaks before recovery.
- Two concurrent HTTP-only runs with different run IDs preserve a dirty-neighbor sentinel.
- Probe actions cannot accept arbitrary URLs, methods, paths, sleep durations, or request counts.
- An action deadline stops the selected profile rather than starting more work.
- Reports contain no bearer token, cookie, configured secret, or synthetic secret marker.
- The authorization matrix covers every protected operation in the supported OpenAPI inventory.
- Ten `smoke` runs and ten `lifecycle` runs pass with zero retries.

## 11. Explicit exclusions

Do not add these cases to this suite:

- Fork bombs, orphan-process tests, or termination-resistant process trees.
- CPU, memory, disk, log, database, queue, or connection exhaustion.
- Filesystem traversal or arbitrary host-file reads by action code.
- Network scans, cloud metadata requests, or arbitrary outbound URLs.
- Worker restart, service restart, migration, or retention tests.
- Direct database assertions or test-only server hooks.
- Public-internet dependencies.
- Generic DAST crawling without an exact target allowlist and a separate operating procedure.

Those cases either test behavior Attune does not promise to restrict or require exclusively owned infrastructure. Keep them in a separately approved operational or penetration-testing exercise.
