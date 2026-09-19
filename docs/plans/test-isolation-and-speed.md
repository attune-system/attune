# Test isolation and execution-speed revision plan

**Status:** Documented implementation plan; transformation not yet started. Findings are a source audit, not benchmark results.  
**Tracking:** Local issue **#60**, “Implement test isolation and execution-speed transformation plan” (`issues get 60`).  
**Scope:** Rust, Python/pack tests, frontend Vitest/jsdom tests, test services, and CI  
**Relationship:** Builds on [test concurrency reliability](test-concurrency-reliability.md), rather than undoing its reliability fixes.

## Recommendation

Keep schema-per-test as the default for database-backed Rust tests. Do **not** simply remove `--test-threads=1`: schemas isolate rows, not PostgreSQL notifications, advisory locks, process globals, RabbitMQ, files, or running services. First make resource ownership explicit, then enable **bounded parallelism by test class**. Optimize fixture provisioning only after measuring it.

The intended outcome is both:

- **Resistance to pollution:** a test passes alone, after another test, and alongside unrelated tests without depending on ambient data, credentials, configuration, clocks, or services.
- **Containment of pollution:** a test cannot delete another test's resources, change its configuration, steal its messages, or leave writers running after teardown.

No test suite, database cleanup, or service restart was run for this audit. Runtime savings below are hypotheses to measure, not claimed speedups. Existing unrelated working-tree changes are outside this plan.

## Current state → intended future state

| Current state | Future state | Evidence required |
|---|---|---|
| Different Make/CI/Docker selections and blanket Rust serialization | Explicit dependency-class lanes and one checked coverage manifest | Same selected test identities/features, with intentional exclusions documented |
| UUID schemas but shared non-DB state and incomplete cleanup | Owned fixture lifecycle for every mutable resource, with isolated exclusive scenarios | Overlapping-run, dirty-neighbor, panic/cancellation and pre-janitor leak checks |
| Repeated full migrations behind a lock; uncertain critical path | Instrumented setup and bounded concurrency; faster provisioning only if justified | Comparable cold/warm phase timings and schema/behavior parity |
| Readiness sleeps, ambient internet/installers and developer config | Observable readiness, controlled dependencies, explicit configuration | Deterministic repeated runs without public-network dependence in ordinary lanes |
| Historical timing claims and recovery masking leaks | CI reports coverage, timing, flake/retry rate and normal-teardown leaks separately | Repeatable wall-clock improvement without weakened assertions or reliability |

Implementation will proceed in the reviewable changesets in section 7; the detailed design and phase gates follow below.

## 1. Findings from the source audit

### Runner contracts differ, and some repeated work is avoidable

| Entrypoint | Observed behavior | Revision |
|---|---|---|
| `Makefile:173-181, 227-249` | Curated integration binaries, separate Cargo invocations, one test thread | Preserve selection; group compatible invocations and classify dependencies |
| `.github/workflows/ci.yml:318-338` | Workspace/all-features, includes ignored tests, explicit skip list, one test thread | Separate pure and infrastructure lanes without dropping coverage |
| `docker/rust-tests-entrypoint.sh:178-193` | Ignored-only execution, four threads; selected package constructed as `attune_${CRATE}` | Reconcile selection and hyphenated package names with workspace |
| Python runners / `tests/pytest.ini` | xdist installed but no default `-n`; API-backed E2E | Certify an isolated subset before enabling workers |
| `web/vitest.config.ts`, `web/package.json:13-14` | Vitest/jsdom with DOM cleanup; no maintained Playwright harness found | Use existing unit runner; do not introduce browser infrastructure just for this plan |

- `docker/Dockerfile.rust-tests:69-74` compiles into a BuildKit cache mount, whose contents are not shipped in the image. Runtime Cargo cannot reuse those artifacts from that layer. The `cargo test ... | tail -30` pipeline can also mask compile failure under the default shell. Ship compatible artifacts/dependencies or a test-executable manifest; make build failure propagate and measure image-size tradeoffs.
- Blocking web CI installs, checks and builds but does not invoke Vitest (`.github/workflows/ci.yml:392-438`). Add unit execution to that job rather than paying for another install job. CI path filters at lines 99–108 omit several test/harness/config-only changes from relevant job selection.
- `tests/helpers/client.py:46-59` retries POST as well as reads on selected server failures. A retried non-idempotent write can create duplicate work and obscure latency/failure rates. Default retries should be read-safe or explicitly idempotency-aware.

### Database isolation is valuable, but provisioning and cleanup need budgets

- `crates/common/src/test_database.rs:22-71` creates a UUID schema, reads/applies migrations for each fixture, and holds a database advisory lock across migration execution. There are 55 SQL migration files in the inspected tree; the fixture explicitly skips the runner-claim migration. Increasing test threads cannot parallelize this locked setup stage.
- `crates/common/src/db.rs:106-124` configures every pooled connection with the schema and `public` search path, plus a schema-specific application name. Retain this per-connection configuration; issuing `SET search_path` on an arbitrary pooled connection is not sufficient.
- Cleanup is **opt-in** at fixture construction. `crates/executor/src/workflow/log.rs:760-804` and the supervisor test starting at `crates/supervisor/src/main.rs:1678` create bare owners without enabling drop cleanup or explicitly calling cleanup. These paths can leave schemas on success, not only after interruption. Fix them first. API setup also transfers ownership to pool/schema fields before a complete context exists (`crates/api/tests/helpers.rs:157-201`), leaving constructor-error paths needing a guard.
- `TestDatabase::cleanup()` is explicit and async. Its fallback `Drop` path starts and joins a cleanup thread, drops the pool owner rather than awaiting all clones, and logs failures. Explicit cleanup waits up to five seconds for the pool, then proceeds to targeted backend termination and schema removal (`test_database.rs:185-239`). This is useful recovery, not proof that background tasks were quiescent.
- `scripts/ci-test-db-safety.sh:52-85` reports leftovers and deletes **all** `test_` schemas/jobs in its target database. A successful janitor can conceal fixture leaks. This is appropriate only in an exclusively owned test database; it is not safe between overlapping runs sharing that database.
- `scripts/cleanup-test-schemas.sh` uses SQL `LIKE 'test_%'`, where `_` is a wildcard, and also has no run ownership filter. Replace broad matching with validated exact ownership, not merely a more precise prefix.

### Schemas do not isolate all PostgreSQL behavior

- Migration triggers publish fixed database-wide channels, for example `execution_created` in `migrations/20250101000005_execution_and_operations.sql:240-258`. That payload has an execution ID but no schema identity. Identical IDs in separate schemas can therefore be ambiguous to a listener. Listener tests need their own database, or a verified namespace-aware notification contract; setting the listener's search path does not namespace channels.
- Extensions, Timescale catalogs, DDL contention, and some advisory locks remain shared. Tests of migration, retention, maintenance, or notification semantics need a stronger boundary than ordinary repository CRUD tests.
- The `public` fallback is needed for extension functions, but must not silently resolve a missing test table to a shared application table. Test roles and fixture validation should make that escape fail closed.

### Non-database resources can still collide

- `crates/worker/src/runtime/process.rs:679-708` writes inline scripts to a shared temp-root `attune/inline_actions/exec_<execution_id><extension>` and later removes that filename. Worker log tests use fixed execution IDs. Their pack TempDirs do not protect this scratch path: another invocation can overwrite or unlink it. Inject an owned scratch root and use exclusively created unique temporary files with guarded lifetime. The naming flaw is observed; a collision was not dynamically reproduced.
- The ignored executor constructor test loads ambient config and initializes default broker topology (`crates/executor/src/service.rs:142-205, 914-926`); `config.test.yaml:19-24` uses the shared `/` vhost. Give real-broker tests an owned vhost and explicit topology config. This constructor declares infrastructure but does not start consumers, so message theft is not an observed behavior of that test.
- `crates/worker/tests/dependency_isolation_test.rs:24-53, 102-250` repeatedly creates real venvs and invokes pip, including `pip>=21.0`. Even the local-package case does not fully forbid index/build downloads. Use an offline pinned wheelhouse and isolated pip config/cache for real installer tests; test command planning without invoking an installer for every case.
- The ignored 1,000-task FIFO stress case (`crates/executor/tests/fifo_ordering_integration_test.rs:412-550`) combines spawn-order expectations, concurrent admissions and fixed readiness delays. Separate load testing from small deterministic FIFO tests, and assert durable enqueue order rather than scheduler spawn order.
- Existing good patterns should be retained: CLI `MockServer`/`TempDir` (`crates/cli/tests/common/mod.rs:7-29`), ephemeral API listeners (`runtime_log_replica_tests.rs:310-312`), UUID notification discriminators (`crates/common/tests/trace_tag_notification_tests.rs:8-64`), and worker/sensor environment restoration guards. Local environment mutexes protect only cooperating callers, not every ambient reader.

### Python E2E ownership is incomplete

- `docker-compose.e2e.yaml:28` and the base Compose configuration use project `attune-dev`, with fixed container names/host ports. `scripts/run-integration-tests.sh:160-172` runs `down --volumes` in its exit trap unless teardown is disabled, including when startup was skipped. This can affect an existing development/other test stack. Merely passing a unique Compose project is insufficient while fixed names/ports remain.

- `tests/conftest.py:143-174` scopes fixture packs to `worker_id` (`test_pack_gw0`, or `test_pack_local`), not to an invocation. Two runs can reuse the same pack. Worker-level sharing also does not make mutable pack contents safe between tests.
- `tests/conftest.py:91-113` changes global retention settings and later restores a snapshot. Parallel xdist sessions can snapshot each other's temporary settings and restore them in the wrong order. Even correct restoration cannot reverse rows already deleted under shortened retention.
- Supervisor retention tests discover an existing `attune`/`public` schema, launch maintenance against it, and restore whole retention configuration tables (`tests/e2e/api/test_supervisor_retention.py:60-87, 118-205`). These belong in an exclusively owned disposable stack/database.
- `clean_test_data` deletes recent events, enforcements, executions, and inquiries by wall-clock window and suppresses errors (`tests/conftest.py:181-218`). The audit found no explicit consumers beyond its definition/documentation, so this is a **latent hazardous helper**, not evidence that every current run executes those deletes. Remove it rather than adopting it for parallel tests.
- `unique_user_client` creates a user but only logs out; `test_pack` has no teardown. A client session ending is not resource cleanup. Ownership tracking or disposal of the containing stack is required.
- `tests/helpers/fixtures.py:806+` can restart a sensor using shared `tests/pids` and `tests/logs` paths. Service restarts affect other tests even if their rows are uniquely named.

### Historical guidance overstates isolation and speed

`docs/testing/running-tests.md` and `docs/testing/schema-per-test.md` still describe parallel execution as universally safe, serial constraints as unnecessary, and historical sub-minute/4–8× performance. Their cleanup examples also differ from the current implementation. The later reliability plan records much more conservative execution and prior local schema accumulation. Treat source/configuration as authoritative until these guides are refreshed; do not use historical times as a baseline.

## 2. Target test classes and isolation boundaries

These are **proposed classifications**, not existing runner guarantees. `cargo test --lib` is not a reliable synonym for “no infrastructure”: classify actual dependencies, including inline tests.

| Class | Resource boundary | Scheduling | Coverage retained |
|---|---|---|---|
| Pure logic | Explicit inputs; no DB, network, process globals | High parallelism | Templates, validation, transitions, permissions decisions, parsing |
| Repository integration | Fresh schema and owned pool in a disposable run database | Bounded DB slots | Real SQL, constraints, transactions, history triggers |
| In-process API | Repository fixture, instance-local auth/cache/audit state, owned temp paths | Bounded API/DB slots | Real routes, `RequireAuth`, repositories, serialization |
| Service/notification integration | Owned tasks/processes; dedicated DB when channels/locks cannot be scoped; RabbitMQ vhost | Small service slots | LISTEN/NOTIFY, dispatch, cancellation, cross-connection visibility |
| Full-stack E2E | Run-owned Compose stack/database/vhost/volumes; test-owned entities | Parallel only for classified non-global cases | Representative sensor → event → rule → execution journeys |
| Global/destructive/migration/load | Exclusive disposable stack or database | Dedicated lane; no overlap with ordinary E2E | Retention, upgrades, shutdown/restart, stress, real timing |
| Browser/unit and pack contracts | Local fakes/mock servers and owned browser contexts/temp paths | Bounded CPU/process slots | UI behavior, stdin JSON contracts, action HTTP behavior |

A serial mutex is only a transitional containment tool. Rust process-local locks do not protect separate test binaries or separate CI jobs; xdist grouping within one run does not protect another invocation. Shared external resources need an external lease or, preferably, separate resources.

## 3. Resource ownership contract

Introduce a small shared fixture contract, not a new general-purpose orchestration framework. Call it `TestScope` here; this is a design name, not an existing API.

Each scope owns:

1. A random **run ID**, worker ID, test ID, and diagnostic label; obey PostgreSQL identifier length limits. Use a mapping/manifest rather than truncating away uniqueness.
2. Its schema/database, DB application names, optional RabbitMQ vhost, filesystem root, ports, created API entities, and task/process handles.
3. Explicit setup, test-body, and teardown deadlines; setup failure also unwinds resources acquired so far.
4. A cancellation/shutdown path and an **awaited** `finish()` that returns cleanup failures.

Teardown order: stop producers/sensors → cancel and join consumers/background tasks → flush/join audit writers → terminate and reap owned subprocess groups → release pools/listeners → remove owned DB/MQ/API/file resources. Capture diagnostics before destroying them. Continue best-effort cleanup after one failure and report all errors without losing the original assertion failure.

Rust async test wrappers should run teardown after a returned error or caught assertion panic where unwinding is available. `Drop` is only a fallback; process abort/SIGKILL requires an external janitor. Python uses yield/finally fixtures with cleanup registered immediately after acquisition. Browser fixtures close their contexts and servers.

The janitor must use an exact ownership manifest/lease and refuse resources of a live run. Never remove “all recent rows”, all `test_*` schemas on a shared server, arbitrary PIDs from stale files, or unowned queues. For dedicated disposable CI infrastructure, destroy the entire owned database/container/volume after recording whether normal teardown leaked.

### Authoring standards

- Create only prerequisites the assertion needs, through existing repository/API fixture builders. Keep DB access in the repository layer; do not introduce another ad hoc service-query path.
- Separate immutable shared inputs (migration SQL, pack archive bytes) from mutable outputs. Copy mutable packs/runtime environments/artifacts under the owned root.
- Make configuration/cache/clock dependencies instance-local. Until process-environment mutation is removed, isolate **readers and writers**, not just writers; restoring a value does not protect a concurrent reader.
- Give spawned commands an explicit environment, cwd, temporary HOME/XDG/config paths, and stdin JSON action parameters. Avoid inheriting developer auth/profile state.
- Bind listeners to loopback port `0` and keep the socket open; “find a free port, close it, then bind” is racy.
- Assert on owned IDs and scoped results, not total table counts, newest rows, or assumed empty global queues. Add deterministic tie-breakers when timestamp order matters.
- Use subscription-ready/admission/completion signals or bounded predicate polling, not sleeps as proof of progress. Record the last observed state on timeout.
- Use virtual time only for in-process logic with controlled clocks. It does not advance PostgreSQL time, subprocess clocks, or a running service. Keep a small real-time contract suite.
- Negative assertions need an observed processing barrier plus an appropriate bounded observation window; simply deleting a sleep can weaken the test.
- Missing required infrastructure fails the declared integration lane. Optional dependencies may skip only in explicitly optional lanes; skips/retries must be visible.

## 4. Phased implementation backlog

### Phase 0 — Establish the baseline and coverage manifest

**Priority:** P0; prerequisite for speed claims.  
**Owner areas:** test scripts, CI, Rust/Python/browser runners, documentation.

- Inventory every entrypoint, selected tests, ignored/skipped tests, feature set, build profile, serialization flag, retries, and service dependencies. Give each test one of the classes above; compare selected test identities before/after runner changes.
- Measure cold build, warm build, dependency startup, fixture setup (lock wait vs migration vs seed), test body, teardown, and total critical-path wall time separately. Report top slow tests and aggregate setup cost.
- Record peak DB connections, migration-lock wait, CPU/RSS/I/O, Timescale/catalog pressure, queue consumers/depth, leftover schemas/tasks/processes/files, and external network calls. Never include secrets in logs.
- Baseline on a fresh owned stack: at least three cold and five warm runs on the same hardware/toolchain. Report individual runs, medians and ranges; collect more repetitions before interpreting p95/p99. Do not compare a warm new run with a cold old run.
- Fix masked Docker build failures and selected package naming; add Vitest to blocking web CI and include harness/config paths in job selection. Establish honest test selection and failure reporting before comparing timing.
- Replace unsupported blanket parallelism/speed claims in the running/schema guides. Link the active lane commands and explain what they omit.

**Exit:** reproducible baseline artifact and explicit coverage/dependency manifest. No change to default concurrency yet.

### Phase 1 — Eliminate destructive/shared-state escape routes

**Priority:** P0 safety prerequisite.  
**Owner areas:** Python fixtures, E2E service scripts, DB janitors.

- Give each invocation an owned database/stack, vhost, temp root, PID/log directory, and unique Compose project/resources. Verify explicit `container_name`, named volumes, networks and fixed host ports cannot defeat project isolation.
- Add run ID to worker pack names; use per-test packs for mutation cases. Track/delete created identities/entities or dispose of the owned stack.
- Remove the time-window deletion helper. Replace snapshot/restore of global retention with exclusive disposable-stack fixtures. Route service restart, maintenance, and destructive tests to that lane.
- Refuse destructive actions without a verified test-resource marker/manifest and expected endpoint identity. Do not fall back to a developer stack when the test target is unavailable.
- Make janitor scope exact and separate **leaks before janitor** from **successful recovery after janitor**. Successful recovery must not turn a fixture-leak check green.

**Exit:** two simultaneous invocations cannot touch each other's resources; cancellation leaves only resources the owning janitor can safely reclaim. Run this gate before adding xdist workers.

### Phase 2 — Finish fixture lifecycle and remove unnecessary waiting

**Priority:** P1, likely low-risk speed and reliability gains.  
**Owner areas:** common/API fixtures and async service tests.

- Reuse the existing owning `TestDatabase` and earlier reliability fixes; do not create a competing database fixture. First fix workflow-log/supervisor bare owners and API partial-construction ownership. Consider cleanup-by-default with an explicit ownership-transfer API rather than an easy-to-miss opt-in. Convert remaining bare-pool/detached-task helpers to scoped ownership and explicit async teardown; bound connect, lock and DROP waits, not only pool close.
- Replace sleep-based readiness with signals/predicates, and real-time timestamp spacing with explicit fixture timestamps where semantics permit.
- Make cache and authorization state instance-local; add two-context tests with colliding primary IDs and different values. Do not validate isolation only by disabling every cache: retain real-cache behavior tests.
- Isolate environment-sensitive and filesystem/process tests, starting with worker inline-script filenames; join subprocess/cancellation helpers and background writers before schema removal. Cancellation tests need a child-start handshake so they prove a running child was terminated.
- Move real pip/venv installation to a bounded offline integration lane with pinned inputs; keep command-construction cases dependency-free. Keep the stress test separate and correct its FIFO oracle before using it as a concurrency gate.
- Replace routine internet-dependent HTTP action tests with a local programmable server for methods, headers, bodies, statuses, timeout and connection-failure cases. Keep public-network smoke tests opt-in, outside the required fast lane.
- Audit `packs/core/tests/run_tests.sh:268-315` and `test_actions.py:55-101, 340-476`: they use httpbin and legacy parameter environment variables. Consolidate overlapping assertions and exercise the canonical **stdin JSON** protocol rather than optimizing an obsolete invocation contract.

**Exit:** affected binaries pass ten repetitions; no unowned background writer or unexplained readiness sleep; teardown failures are surfaced. Local HTTP contract tests pass with public internet unavailable.

### Phase 3 — Enable bounded parallel lanes

**Priority:** P1 after Phases 1–2.  
**Owner areas:** CI/Makefile, Rust runner, pytest/browser configuration.

- Start with pure logic, then audited repository/API binaries at 2 and 4 concurrent slots. Tune further only from measured throughput and resource saturation.
- Bound total DB demand: active fixtures × fixture pool cap + setup/admin/listener/service connections + safety reserve must stay below the test server's connection budget. Include every test process/job, not only one binary's thread count.
- Keep the existing migration lock initially. Measure whether it becomes the critical path; moving work behind a global lock is not scalable parallelism.
- Evaluate `cargo-nextest` for per-test scheduling, resource groups, timing, and process isolation, but do not make adoption a prerequisite. Verify toolchain/features and test enumeration; run doctests separately if the chosen runner excludes them. Per-test processes isolate Rust globals, not external resources, and may repeat process-level setup.
- Keep notification/maintenance/global-mutation cases on dedicated resources. Do not “fix” cross-schema notifications by filtering only colliding numeric IDs.
- Preserve explicit integration/stress coverage while removing blanket serialization only for approved classes. Do not count fewer selected tests or increased retries as an optimization.

**Exit:** repeated parallel runs pass the isolation matrix in section 6, with an unchanged coverage manifest and measured improvement over the serial baseline. Unsafe classes remain isolated/serial.

### Phase 4 — Reduce database fixture cost, if profiling justifies it

**Priority:** P2; higher implementation risk.  
**Owner areas:** common fixtures, migrations, repository test seams.

Evaluate in this order:

1. Cache immutable migration text/path ordering within a test process or embed it; measure first. This saves filesystem work, not SQL/DDL cost.
2. Seed only the rows needed by each test; share immutable seed definitions, never a mutable seeded schema across independent tests.
3. For genuinely single-connection repository operations, consider a transaction-bound repository executor and rollback fixture. This requires existing APIs to accept the executor cleanly. It is unsuitable for pool-escaping work, independent commits, listeners, cross-connection visibility, and `NOTIFY`-on-commit behavior. Keep ordinary real-schema tests for those contracts.
4. Prototype a pre-migrated **database template**, cloned into disposable worker/test databases, only if migration time dominates. Key it by migration/seed digest, PostgreSQL/Timescale versions, and fixture settings; ensure the template has no active sessions and validate extension/catalog/hypertable correctness. Account for database-create privileges, storage and clone contention.
5. Never approximate Timescale migration state by naïvely copying tables with `LIKE INCLUDING ALL` or regex-renaming a schema dump. Triggers, functions, sequences, policies, continuous aggregates and extension metadata need fidelity checks.

A database per worker still needs schema/transaction isolation between its tests; a template is not permission to share mutable rows. Resetting one reused schema with `TRUNCATE` is not the default: it can miss jobs, sequences, config, cached state and background writers. Preserve a fresh full-migration lane, including production migration-runner behavior that the test fixture skips, even if ordinary fixtures use a faster snapshot.

**Exit:** benchmarked end-to-end gain, schema/behavior parity, repeated isolation passes, and continued full migration coverage. Abandon the optimization if cloning/resetting is more complex or slower than migration.

### Phase 5 — Shrink full-stack dependence and finalize entrypoints

**Priority:** P2, incremental.  
**Owner areas:** service domain logic, API tests, web/pack tests, test docs.

- Move combinatorial validation, workflow decision, retry/backoff, permission-decision and scheduling-policy cases to deterministic pure tests. Keep representative real repository/API/service boundary tests; mocks do not replace SQL/auth/MQ contracts.
- Retain a smaller representative full-stack journey suite; run destructive, migration, load and public-network tests in separately declared lanes with appropriate PR/nightly/release policy.
- Build required binaries once per compatible feature/profile/toolchain and pass their paths to harnesses; avoid repeated `cargo run` or rebuilding the full stack for each scenario. Fix Docker artifact persistence rather than assuming BuildKit caches are image contents. Repeated Cargo invocations incur planning/startup but do not necessarily recompile unchanged code. Preserve the existing linker-related test-profile workarounds unless independently validated.
- Publish supported commands for fast local, integration, full-stack and exclusive tests. Add selection checks so Makefile and CI do not silently diverge.

**Exit:** every moved scenario has a coverage mapping, each lane has an honest prerequisite list, and CI reports timings, skips and leaks separately.

## 5. Priority and expected benefit

| Change | Expected benefit (unmeasured) | Risk / constraint |
|---|---|---|
| Signals instead of readiness sleeps; local HTTP servers | Lower fixed latency and fewer flaky retries | Must preserve timeout/negative-assertion coverage |
| Awaited ownership/teardown and exact janitors | Stops catalog/task/resource accumulation across runs | Requires panic/cancellation testing |
| Run-level E2E isolation and exclusive global lane | Enables trustworthy concurrent runs | More infrastructure/storage per run |
| Instance-local state plus bounded parallelism | Likely broadest wall-clock improvement | DB/MQ/CPU budgets, locked migrations |
| Smaller explicit seed fixtures and build reuse | Reduces repeated work | Must preserve realistic invariants |
| Transaction fixtures or database templates | Potentially large setup reduction | API changes or Timescale/template complexity; benchmark-gated |

Do the safety prerequisites first, then choose speed work using the measured critical path—not the number of tests changed.

## 6. Validation and rollout gates

For each converted class, run:

1. Each affected test alone, then the whole binary/class in ordinary and shuffled/reversed order where the runner supports it; record the order/seed.
2. At least ten repetitions at concurrency 1, 2 and 4, followed by the selected CI budget. Also test under constrained CPU/DB resources; do not replace readiness signals with larger sleeps.
3. Two independent invocations concurrently, using separate run IDs. Include tests that intentionally create equal primary IDs, equal logical names in isolated scopes, and notification traffic in neighboring schemas/databases.
4. A controlled dirty-neighbor fixture containing sentinel data/config/resources. Assert byte/value identity afterward; neither run's cleanup may remove the neighbor.
5. Inject assertion panic, partial setup failure, cancellation, and process termination. Verify normal teardown or exact janitor recovery; no non-owned PID/backend is terminated.
6. Compare baseline and post-test resource identities/counts **before** janitor recovery, then verify final cleanup. Counts alone are insufficient if one resource was deleted and another leaked.
7. Run the CI-equivalent complete suite and fresh migration lane. For implementation changes run targeted checks plus `cargo check --all-targets --workspace`, formatting and relevant frontend/Python checks. Schema changes also require `cargo sqlx prepare`.

**Acceptance:** no reduced coverage, no retries hiding failures, no changed foreign resources, no unexplained leaks after successful tests, and repeatable median wall-clock improvement without worse tail reliability. Choose an explicit speed target after Phase 0; no numeric speedup is promised by this audit. Ten clean runs are a rollout gate, not proof that races cannot exist—continue tracking flake rate in CI.

Roll out lane by lane behind configurable concurrency. If a class fails, restore its prior scheduling and keep its ownership fixes; quarantine only with a recorded reason/owner. Do not restore shared destructive cleanup or globally serialize already-proven pure tests.

## 7. Execution sequence and handoffs

Deliver small reviewable changesets, not one workspace-wide rewrite. The implementer records changed paths, validation commands/results, measured timing and residual risks on the implementation issue after each changeset. Use CI artifacts for raw benchmark output and a concise checked-in results section here for durable conclusions; never publish credentials or unredacted connection URLs.

| Order | Changeset / primary paths | Dependency and completion gate |
|---|---|---|
| 1 | **Safety prerequisites:** `scripts/run-integration-tests.sh`, Compose test configuration, DB janitors, `tests/conftest.py` | Before running destructive baselines: explicitly owned target, non-owned stack teardown refused, retention/restart scenarios exclusive. Implement Phase 1 containment without raising concurrency. |
| 2 | **Reliable baseline and runner contract:** Makefile, `.github/workflows/ci.yml`, Docker test image/entrypoint, test timing helpers | Phase 0: make failures/selection honest, persist intended artifacts, run Vitest, inventory coverage and phase timings on the owned infrastructure. Set a measurable speed target from this baseline. |
| 3 | **DB ownership:** `test_database.rs`, API helpers, workflow-log and supervisor fixture callers | Phase 2: constructor-failure guard, explicit awaited cleanup and deadlines, regression tests for success/panic/partial setup. No successful-test leftovers before janitor. |
| 4 | **Non-DB ownership and deterministic tests:** worker scratch/process tests, API cache policy, service test tasks, Python/pack fixtures | Phase 2: owned paths/vhosts/accounts, instance-local state, readiness signals and offline HTTP/installer tests. Prove overlapping invocations cannot interfere. Split independently safe areas into separate reviews. |
| 5 | **Bounded parallel pilot:** classified pure tests, then representative common/API integration binaries; runner resource limits | Phase 3, after changesets 2–4 for the selected class: test at 1/2/4 slots, run section 6 gates, compare identical coverage. Preserve existing migration lock and exclusive lanes. |
| 6 | **Measured critical-path optimization:** fixture migration loading/seeding or compatible build reuse | After pilot profiling: implement low-risk wins first. Transaction fixtures/database templates require a separately recorded benchmark/design decision and parity tests; explicitly defer them if not justified. |
| 7 | **Expand and publish:** remaining certified lanes, representative E2E, `docs/testing/*`, results in this plan | Phase 5: full CI-equivalent/migration validation, final before/after report, documented commands and remaining exclusive/serial exceptions. |

Changeset 1 precedes destructive measurement because the existing E2E runner can target shared development resources. Read-only inventory can begin immediately. Later changesets may proceed per isolated test class once that class's prerequisites pass; there is no requirement to wait for every service before piloting pure-test parallelism.

### Final handoff / issue closure

- [ ] Baseline and final measurements use the same coverage, hardware/toolchain and comparable cold/warm conditions; the baseline-derived performance target is met, or the issue remains open with measured blockers.
- [ ] Normal teardown and failure recovery pass separately; unrelated sentinel resources are unchanged, including during overlapping runs.
- [ ] Approved lanes use bounded resource budgets; remaining serial/exclusive cases have documented reasons and owners.
- [ ] SQL, auth, real service boundaries and full migration coverage remain exercised; skips and retries are visible.
- [ ] All implemented phases have their targeted checks and workspace validation recorded, with no new warnings.
- [ ] Optional provisioning experiments are either validated and adopted or explicitly deferred with evidence; speculative template/transaction work is not required for closure.
- [ ] Running guides reflect implemented commands rather than historical guarantees, and the issue links the final results.

