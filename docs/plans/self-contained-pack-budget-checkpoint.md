# Managed-pack rework budget checkpoint

Date: 2026-10-16

Update: the user subsequently authorized a bounded release-stabilization pass
on the current branch. The full SOW remains deferred. That pass fixes startup
and existing-flow regressions and validates the supported release scope; it does
not restart the paused implementation lanes or require every SOW feature to ship.
The pause evidence below remains historical. The branch is not yet release-ready.

Implementation is paused at the user's request. Further work requires explicit
prioritization and budget approval. This checkpoint supersedes the active-work
language in earlier progress and coordination notes.

## What is retained

The primary checkout is on `feature/named-type-schemas`, HEAD
`1fa52e22a077fea1a5ee86d259e038023234b76a`. Its integrated changes remain
uncommitted. No primary commits, pushes or deployments were made for this pause.
All task worktrees and their local commits remain available. No work was reset
or discarded. Unrelated SDK and Docker changes remain untouched.

The latest primary code includes the earlier admission/ownership/configuration
and native-input work, H's private ResultView caller, the targeted production
cache-future boxing fix, and seven measured caller/fixture repairs. Temporary
stack instrumentation was removed; the bounded public-future regression remains.

This is a buildable development checkpoint, **not a release-ready or fully
validated implementation**. The exact original cross-pack deployment scenario
has not received a demonstrated end-to-end pass. Supported-flow gaps and earlier
regression failures must not be hidden by the successful build.

All readiness, issuance, registrar-exemption, bootstrap-activation and production
gates remain off. The full 20-criterion SOW is incomplete.

## Validation evidence and limits

| Evidence | Result | Limit |
| --- | --- | --- |
| All-target workspace check after diagnostic cleanup and seven repairs | Passed without warnings, 121 seconds | Not runtime acceptance |
| `ct184042f1d8`, original empty-cache caller | 1 passed on PostgreSQL 18.6 | Before later probe cleanup and seven repairs |
| `ct1854155c44`, cache activation module | 8 passed, zero failed/ignored | Six actual caller cases and two pure checks; before cleanup and repairs |
| Production future measurement | Cache public future 287,664 to 16 bytes; fixture 954,856 to 246,480 bytes | Layout measurement, not a general stack-use proof |
| `ct19001658c6`, common/executor regression attempt | Selected targets built; existing migrations applied on PostgreSQL 18.6 | Authority test process hung for over 84 minutes; interrupted, no completed test summary |

The interrupted runner's `finally` cleanup stopped
`attune-consumers-ct19001658c6`. A subsequent container check found no running
container with that name. No additional builds or live services were started for
this checkpoint. The interrupted suite is inconclusive, not green. Its later
admission, retention and executor selections did not receive completed results.

The runner buffers output and has no test-process timeout. Ticket #114 covers
bounded execution, partial-output preservation and diagnosis before any rerun.

## Deferred tickets

The repository uses the local `issues` CLI and `.scratch/issues.db`, not GitHub
issues. Umbrella **#112** tracks the pause. All children carry `budget-deferred`
and `needs-triage`, not `ready-for-agent`. These tickets do not authorize work to
restart automatically.

| Ticket | Deferred work | Key remaining requirement |
| --- | --- | --- |
| #113 | Original cross-pack event-to-execution binding | Real two-pack deployment/execution positive and unauthorized negative; distinguish ordinary and secret event values |
| #114 | Stalled PostgreSQL authority suite | Bounded runner, identify stuck test/lock, then complete focused current-checkpoint regressions |
| #115 | Incremental source integration | Choose necessary batches, preserve exact prerequisites, compile and validate each before the next |
| #116 | Historical success leaves | Core, canonical projection guard, mandatory finalizer, C/F archive adaptations, real GC/expiry acceptance |
| #117 | Projected empty/Wait/cache programs | Finish genuine adapters and task-state projection without rebasing original input authority |
| #118 | Wait root input/readers/custody | Separate root, observer and program scopes; actual recorded/Access/transport/GC/root-expiry cases |
| #119 | Controlled native topology/lifecycle | Real measured artifacts and all seven native plus three systemd modes; none has run |
| #120 | Requested broker acceptance | Six real API cases plus one workflow-retry case, zero ignored, owned five-vhost teardown |
| #121 | Distinct-principal interoperability | Real API/CLI/MCP/SDK/browser and deployment/upgrade evidence, no helper-only substitutes |
| #122 | Final SOW/release reconciliation | Actual evidence for all accepted criteria before any gate activation |

Start any future functional work with #113. A decision to finish the wider SOW
is separate from fixing that original scenario. #114 blocks a claim that the
current integrated checkpoint passes its regression suite.

Use `issues get 112` and `issues list --parent 112` for full ticket bodies. Each
ticket records the relevant commits, artifacts, known gaps and acceptance criteria.

## Preserved source, not applied feature bundles

The following directories are under `/tmp/opencode/sow-f067e2bb/`. Each contains
an immutable reviewed manifest and `HANDOFF.md`. Their hash/parse/apply-check
results are source evidence only. None of these newer bundles was applied to the
primary checkout during the budget pause.

| Bundle | Paths | Scope |
| --- | ---: | --- |
| `integration-native-cache-wait-readers-round8-complete` | 87 | Complete held owners, finite cache caller/readers/custody, active inquiry 01030 |
| `integration-success-empty-readers-round9-complete` | 106 | Adds ordinary success 01027, empty iteration 01028 and success readers |
| `integration-success-empty-custody-round10` | 112 | Adds transitive cache fence and cache/success physical test sources |
| `integration-typed-wait-output-round11` | 121 | Adds wait output 01029, actual consumer/replay/fixture, typed readers/custody |
| `integration-operational-acceptance-round12` | 195 | Adds native topology 06016 and E/G/I operational drivers |

Do not apply the full 195-path bundle merely to include completed source. Later
historical-leaf, root-input and projected-program changes are not all in it.
Whole-file snapshots from older worktrees can overwrite integrated fixes. Future
integration must use exact own deltas and refresh original hashes.

## Worktree recovery index

Worktrees are under `/mnt/wdc/attune-worktrees/sow-f067e2bb/`. Short commit IDs
below identify preserved local commits, not main-branch integration.

| Worktree/lane | Last reported HEAD | Important preserved state |
| --- | --- | --- |
| A, `a-policy` | `42a5040e` | `390c5fae` transitive cache fence; `8e0e95e8` historical core/01031; `aab9ec91` projection guard; `42a5040e` genuine expiry test; clean |
| B, `b-execution` | `4c401d7d` | Projected capability `7b596ff2`/`4c401d7d`; dirty Wait refactor, cache structural and older executor work remain unfinished |
| C, `c-consumers` | `d44ee79b` | Typed cache/success/Wait readers and historical adaptations; staged root-third-scope adaptation in `native_input_disclosure/wait_completion.rs` remains unfinished |
| D, `d-native-iteration` | `82dd19bb` | Complete empty-recipe delta; dirty forest/adapters and untracked `native_iteration_materialization/forest.rs` are not integration-ready |
| A3, `a-wait-integrated` | `0edfd7e7` | Wait recipe `5d3037d2`, audit `eabaa26e`, separate root input `0edfd7e7`; clean |
| J, `j-staged-control` | `316853ee` | Actual consumer `6de7fea1`, owning fixture `2e2d3acd`, root-Key fixture `7cb95da3`, closed adapter `0de1a9af`/`316853ee`; clean |
| H, `h-native-completion` | `924f2a94` | Caller `0efdb725`, required finalizer `2488d6b6`, real finite Key-leaf fixture `924f2a94`; clean |
| F, `f-completion` | `b60bc2e1` | Topology `11dea69e`, wait cases `ae285d43`, history custody `f942a700`, root custody `b60bc2e1`; clean |
| E | `30962e48` | Actual topology adoption after `6ef0d1d5`/`d7cc4001`/`1c7fcd9e`; application/charts clean, user Docker edit preserved |
| G | `df7e12b4` | Real-client follow-ups `b79a075d`/`df7e12b4`; clean |
| I | `0524f0dc` | Broker range `0d1fc171..0524f0dc`; clean, resolved stage retained |

Some worktree names come from historical lane assignments. The exact worktree
paths are recorded in `budget-pause/worktrees.txt` and the coordination ledger.
Confirm actual HEAD/status before editing.

`budget-pause/preservation.json` records primary and dirty B/C/D snapshots,
including separate HEAD and index patches, status files and untracked-file
archives. This preserves C's staging state and D's untracked forest. Original
worktrees are still the source of truth. These local backups and the local issue
database are not a substitute for a separately authorized repository backup.

Do not deliver prerequisite-only A3 snapshot commits `46735232`, `b5223c01` or
`a4e2f100` as feature commits. Deliver only the documented own recipe increments.
Do not rewrite applied migrations or assume a reserved number is implemented.
`01032` was reserved but unused. Preserve the existing 06005/06015 hashes.

## Budget boundary

This pause added tickets, preservation artifacts and this status record. It did
not restart feature implementation, apply the uncompiled bundles, enable gates,
push code or deploy services. Future work should use one selected ticket and a
bounded validation plan rather than resume every lane.
