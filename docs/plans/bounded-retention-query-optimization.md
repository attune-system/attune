# Bounded retention query optimization plan

Status: proposed; implementation and deletion validation have not started.
Date: 2026-11-09 UTC (2026-11-08 America/Chicago).

## Objective and scope

Make retention candidate discovery, query planning, and retry scheduling fit
Attune's bounded maintenance budgets as the database grows. Preserve current
retention policies, live references, evidence pins, authorization, and physical
cleanup fencing.

The investigation used the deployed Attune `0.10.3` release. Reconcile the
findings with the implementation branch before changing code. This document
records the investigation and proposed work; it does not authorize a database
reset, deletion outside retention policy, release, or deployment.

Coordinate transaction ownership and admission changes with the
[graph admission contention plan](graph-admission-contention.md),
[database activity reduction plan](database-activity-reduction.md), and
[transaction reuse remediation plan](database-transaction-reuse-remediation.md).
Preserve the current v1 contracts and use forward migrations if new durable
scan state is necessary.

## Observed behavior

The supervisor emitted `core.alert` events with failure type
`graph_reclamation_deferred`. PostgreSQL logs showed statement timeouts in
release reclamation and expired-artifact discovery. Supervisor logs included
`CommitUnknown` results and repeated incomplete maintenance cycles.

At the investigation snapshot:

- The database occupied approximately 337 MB on a 40 GiB PostgreSQL PVC, with
  roughly 38 GiB free. This was not an immediate capacity emergency.
- There was one active core release and no inactive releases. No execution rows
  were older than their configured retention period.
- Twenty ledger entries were `uploading`, all younger than the configured
  24-hour abandonment threshold. They were not overdue object deletions.
- Runtime retention was enabled, with dry-run disabled and a nominal hourly
  cadence. Most runtime targets retained 30 days; audit events retained 90 days.
- Artifact retention used seven days for the runtime-log artifact and four
  versions for two other artifacts.
- Release and object-retention progress remained deferred. Artifact cleanup
  also repeatedly timed out, preventing its maintenance job from completing.

These are point-in-time observations, not evidence that retention will keep up
once data becomes eligible. Age-based retention is not a database byte quota.

## Query and planning findings

### Release query planning dominates an empty cleanup pass

Source: [release predicates](../../crates/common/src/repositories/pack_retention/queries.rs)
and [bounded collector](../../crates/common/src/repositories/pack_retention/bounded.rs).

The release query combines rollback-age and newest-release protection with a
large collection of relational and JSON dependency checks. PostgreSQL can pull
the `NOT EXISTS` checks into a large anti-join plan. Nested `jsonb_each` calls
produce particularly large row-count estimates.

One plan estimated approximately 160 million intermediate rows for nested
static-action dependencies. The live data contained thousands of schema
bindings but no expanded static-action entries at the measurement snapshot.
Estimated cost nevertheless triggered JIT compilation.

JIT means just-in-time compilation: PostgreSQL compiles parts of query execution
into machine code. The observed release-query reproduction generated 164 JIT
functions and spent approximately 11.9 seconds compiling them. That cannot fit
the default 250 ms statement and one-second page budgets. Graph collection also
has a separate ten-second cycle budget.

Disabling JIT removes that compilation cost, but does not reliably eliminate
planning overhead. A prepared custom-plan reproduction still spent about
241 ms planning the combined query, leaving almost no statement-budget margin.
Keeping existence checks correlated with `OFFSET 0` reduced that reproduction's
planning time to about 12 ms. Both custom and generic prepared plans improved.
This is a tested query-shape technique, not a guarantee for every phase or data
distribution.

A materialized CTE can fence execution order, but PostgreSQL still plans the
entire statement. Avoiding unnecessary planning of the dependency query requires
an application-level boundary between coarse eligibility and dependency work.

### Artifact LIMIT does not bound discovery work

Source: [artifact maintenance repository](../../crates/common/src/repositories/maintenance.rs),
especially `expired_artifact_candidates_bounded`,
`expired_artifact_version_predicate`, and `claim_expired_artifact_version`.

The current discovery query combines expiration and
`file_evidence_version_eligible(av.id)` in one predicate, then applies an output
`LIMIT`. PostgreSQL evaluated evidence eligibility across thousands of versions
before excluding unexpired data. Returning no candidates therefore required a
large scan; the output limit did not bound rows visited or function calls.

Materializing expiration candidates before evidence checks substantially reduced
work in the observed empty-result case. A full expiration scan still scales with
table size, however. Durable pagination is required for bounded work at larger
sizes.

Version retention also counts every newer ready version even though it only
needs to establish that at least the configured number exist. An indexed probe
can stop when that threshold is reached.

### Measurements and limits

The following totals sum median planning and median execution times from three
read-only `EXPLAIN ANALYZE` runs per variant. JIT was disabled only in the test
transactions. Every current-data query returned no eligible candidates.

| Prototype | Planning | Execution | Combined |
| --- | ---: | ---: | ---: |
| Combined release eligibility/dependency query | 91.8 ms | 7.6 ms | 99.4 ms |
| Cheap release eligibility probe | 3.1 ms | 2.5 ms | 5.6 ms |
| Release query with correlated existence checks | 11.3 ms | 2.4 ms | 13.7 ms |
| Original artifact discovery, full scan | 2.1 ms | 1,752.4 ms | 1,754.5 ms |
| Expiration-first artifact discovery, full scan | 2.7 ms | 22.1 ms | 24.7 ms |
| Bounded artifact discovery, one 256-row page | 3.0 ms | 3.1 ms | 6.1 ms |

These measurements are not identical amounts of work: the release probe is only
a first stage, and one artifact page is not a complete pass. The full-scan
artifact prototype also replaced count-all-newer with a bounded existence probe;
its measured improvement is for the combined rewrite. Measurements varied with
database load and planning/cache state. The release prototype protected three
newest inactive releases; the deployed default protects two. There were no
inactive releases, so this did not exercise that distinction.

Read-only candidate comparisons found no differences for:

- Current data: both complete queries returned zero candidates.
- A simulated cutoff 30 days later: both returned the same first 100 candidates.
- Generated time-policy, version-gap, body-state, zero/negative-limit, and
  unsealed-log cases: both predicates selected the same 88 rows.
- A complete paged traversal under the simulated future cutoff: 36 pages through
  a frozen high-water ID of 8,971 produced the same first 100 candidates.

These checks validate candidate-selection prototypes, not concurrent deletion
or a production-ready collector. No production rows, schema, or settings changed.
Local SQL, plans, and machine-readable results are retained under
`attune-system/.scratch/retention-investigation/`; this plan embeds the essential
findings and must remain understandable without those untracked artifacts.

## Proposed implementation

### 1. Keep JIT out of bounded maintenance transactions

Apply `SET LOCAL jit = off` at transaction entry in the bounded maintenance
paths, including discovery, protected work, and separately owned bookkeeping
transactions where applicable. Audit `maintenance_admission`, the graph
collector, and the metadata portions of `PhysicalCleanup` for coverage.

Keep the setting transaction-local so pooled connections and unrelated service
queries do not inherit it. Preserve ownership through commit or acknowledged
rollback, and dispose of connections with uncertain state.

JIT may help long CPU-heavy maintenance operations when execution savings exceed
compilation cost. That benefit has not been demonstrated for these short bounded
units. Benchmark any future large-batch exception explicitly; retain the existing
deadlines rather than relying on compilation completing inside them.

### 2. Stage release eligibility before dependency checks

Retain the collector's bounded raw-ID window, frozen high-water mark, and
captured row identities. Under the existing exclusive graph admission and a
fresh snapshot:

1. Recheck row identity, inactivity, rollback age, active-release exclusion, and
   protection for the configured newest inactive releases.
2. If no coarse candidate survives, atomically advance the raw scan state without
   submitting the dependency query.
3. For survivors, evaluate all current evidence, normalized dependency, live
   component, direct execution, and JSON snapshot pins. Keep probes correlated
   or split them into small query groups where plans show a benefit.
4. Recheck final eligibility and lock the selected rows in the protected delete
   transaction. Commit cursor progress with the corresponding metadata work.

Discovery results are advisory. An unlocked empty probe cannot by itself
authorize deletion or establish that a protected page has completed. Do not
remove JSON checks merely because a normalized dependency table exists; prove
coverage of every writer and retained snapshot before changing that contract.

Test the existing seven reclamation phases independently. Splitting release
eligibility must not silently change policy-revision retention semantics.

### 3. Add bounded, resumable artifact discovery

Use a primary-key cursor and frozen high-water mark to select at most one raw
page before evaluating expiration or evidence. Reuse the existing bounded-scan
patterns and durable progress infrastructure where their semantics fit.

Within a page:

1. Materialize the raw candidates.
2. Materialize candidates whose current policy and body state satisfy expiration.
3. Apply evidence eligibility only to those expiration candidates.
4. Return raw-visit count, cursor/high-water state, eligible candidates, and any
   unresolved failure separately.

Advance according to the raw records visited, not the number eligible. A page
containing only pinned or unexpired records must not starve later IDs. An empty
eligible result must not mark the entire pass complete. Wrap only after visiting
the frozen high-water range, and revisit earlier records on a later pass.

Preserve deterministic candidate ordering within a page. If the API requires
globally oldest-first discovery, either preserve that contract with an
appropriate cursor or explicitly separate bounded scan order from processing
order. Do not assume primary-key order is identical to creation-time order.

Keep `claim_expired_artifact_version` authoritative: after exclusive graph
admission and parent locking, recheck current policy, body state, version
frontier, active-stream constraints, and evidence eligibility before claiming
physical cleanup. Preserve the existing physical-work fence and settlement rules.

### 4. Bound version-retention probes

Replace `COUNT(all newer ready versions) >= retention_limit` with a probe that
stops once the configured number of newer ready versions is established. One
tested shape is:

```sql
EXISTS (
    SELECT 1
    FROM artifact_version newer
    WHERE newer.artifact = av.artifact
      AND (newer.body_state IS NULL OR newer.body_state = 'ready')
      AND newer.version > av.version
    ORDER BY newer.version DESC
    OFFSET GREATEST(a.retention_limit - 1, 0)
    LIMIT 1
)
```

Retain the outer positive-limit condition and the existing active-log rule.
Preserve version gaps and the exact body-state definition. A per-artifact frontier
computed once per page may reduce repeated work further, but needs its own
equivalence and concurrency tests before adoption.

Existing `(artifact, version)` and release-retention indexes already support
important probes. Start by changing query shape. Add partial or composite indexes
only when representative plans demonstrate a missing access path; account for
index storage and write amplification. A raw candidate cap does not bound the
cost of a dependency traversal, so oversized pinned graphs still need explicit
budget and revisit handling.

### 5. Separate continuation, deferred work, and completed passes

Sources: [supervisor scheduling](../../crates/supervisor/src/main.rs),
[durable job scheduling](../../crates/common/src/repositories/native_maintenance/schedule.rs),
and [graph phase scheduling](../../crates/common/src/repositories/pack_retention.rs).

An unfinished metadata scan currently requests rapid continuation. A failed
graph phase can keep the overall object-retention job deferred while other scans
continue or restart. Overall job deferral age is also folded into metadata
deferral reporting, so the reported age need not identify a metadata-specific
failure.

- Schedule independently due phases from their durable state. Avoid restarting
  unrelated completed scans just because another phase remains deferred.
- Use rapid continuation for unfinished raw visits making progress; use bounded
  completion-time backoff for lock conflicts, timeouts, and failures.
- Distinguish successful no-work passes, advancing scans, and unresolved failed
  visits in metrics and alert age. Do not equate scan continuation with overdue
  eligible data.
- Preserve the oldest unresolved failure across cheap successes and dirty passes.
  Clear it only after the relevant clean pass or documented recovery condition.
- Preserve unknown-commit handling. A timeout or missing acknowledgement cannot
  authorize physical cleanup or justify resetting progress manually.

## Validation and acceptance criteria

### Candidate semantics and scaling

- Match existing candidate sets on an isolated database for time policies, version
  gaps, body states, active logs, changed policies, pins, and release protections.
- Exercise nonempty inactive-release catalogs, newest-inactive values including
  zero and the deployed default, equal-time ordering, and rollback boundaries.
- Measure growing artifact tables with no expired rows and with eligible records
  behind multiple pages of pinned records. Bound raw visits independently of
  output count and demonstrate eventual traversal under continuous insertion.
- Test long version histories and large nested dependency graphs. Verify index
  use and avoid substituting a full traversal for a bounded threshold probe.

### Planning and transaction safety

- Measure planning, execution, JIT, buffers, rows visited, and dependency function
  calls separately. Include cold and warm runs, custom and generic prepared
  plans, and realistic contention/storage latency.
- Fit the existing statement/page/cycle budgets with meaningful headroom on
  supported deployment hardware. Report exceptions rather than masking them by
  increasing timeouts globally.
- Verify transaction-local settings reset after commit and rollback. Test
  cancellation, discarded connections, and unknown commit outcomes.
- Test concurrent pin creation, publication, policy changes, and active-stream
  changes between discovery and claim. Live and pinned data must survive.
- Test restart, durable cursor wrap, failed singleton revisit, adaptive page
  sizing, oldest-deferral preservation, and completion after all work is visited.
- On disposable fixtures, prove that eligible metadata and physical objects are
  actually removed and that ledger/evidence cleanup completes. Read-only plan
  tests alone are insufficient.

### Operational handoff

After implementation and required CI, prepare a separate deployment with retained
recovery material. Verify real phase progress, successful job completion, healthy
retry cadence, and absence of repeated timeout-driven alerts. Observe both idle
and active periods; do not use a quiet interval alone as proof of retention.

Track database, index, WAL, and object-store usage alongside incoming workload and
retention age. Confirm that the configured 30/90-day windows fit available
capacity. A successful retention fix does not establish that a 40 GiB database
volume can hold any workload for those periods. Choose policy or capacity changes
separately; do not silently shorten retention to make the charts look healthy.

## Suggested work sequence

1. Add representative isolated fixtures and planner/candidate measurements.
2. Implement transaction-local JIT control and staged release selection.
3. Implement bounded artifact discovery and bounded version probes.
4. Correct phase continuation and failure reporting using measured behavior.
5. Complete deletion, race, recovery, and scaling validation; document remaining
   supported limits before preparing a release or deployment.
