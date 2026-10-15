# Artifact-read acceptance and rollout

The [implementation checkpoint](../plans/artifacts-postgresql-implementation.md)
records local results and incomplete gates. A passing query-count test does not
certify page latency, deployed behavior, or storage capacity.

## Executable local gates

```bash
python3 scripts/benchmark-artifact-reads.py --stage treatment --versions 16 18 \
  --snapshot-source --with-key-usage-tests --with-notifier-tests \
  --workload-profile --paired-reference --notifier-profile \
  --output /tmp/opencode/artifact-read-acceptance
```

`--paired-reference` mounts two test-only paths under the same Axum router. One
uses the production execution-artifact handler. The other deliberately performs
serial per-artifact/per-execution reads while calling the same repository
visibility and runtime-log disclosure functions. A routine differential test
checks eligible pages and missing-origin denials before the performance selection.
The reference is not part of the production router or an alternate API format.

The paired selection keeps two logs, 100 retained executions, caller grants, and
responses identical. It records separate auth-cache-invalidation samples, then
alternates reader order over 20 complete warm samples each. The runner fails unless
all bodies match, the serial reference exceeds the 20-statement ceiling, the batched
reader stays within it, queries fall by at least 90%, and warm handler p95 falls by
at least 70%. Failures and unchanged-source checks are retained in the manifest.
These are controlled batching results, not a reconstructed historical build,
browser/network latency, or certification of the deployed revision.

`--notifier-profile` measures real artifact-visibility queries and fan-out for
1/16/64 permitted identities, each with 1/4 tabs, plus a denied identity. It checks
once-per-fingerprint evaluation, complete permitted delivery, and no denied
frames. SQL counts and authorization wall time are separate from artifact-handler
metrics. The workload profile separately records explicit audit emission/drain
time and the audit-row delta, which can include HTTP audit events.

## Retention and backlog forecast

`scripts/artifact_read_capacity.py` performs read-only arithmetic from sanitized
measurements. Supply the sample interval, insertion counts, a representative row
and storage sample, each cohort's retention/backlog horizon, whole-volume capacity,
all bytes outside the modeled cohorts, and an explicitly chosen reserve/safety
policy. Do not include secret bodies or SQL binds.

```json
{
  "sample_seconds": 3600,
  "volume_capacity_bytes": 100000000000,
  "unmodeled_used_bytes": 20000000000,
  "reserve_fraction": 0.25,
  "storage_safety_factor": 1.5,
  "cohorts": [
    {
      "name": "artifact-version-metadata",
      "inserted_rows": 10000,
      "sampled_rows": 10000,
      "sampled_total_bytes": 20000000,
      "retention_seconds": 604800,
      "backlog_seconds": 86400
    }
  ]
}
```

Those numbers are an input example, not deployment measurements or a recommended
reserve policy. Include table/index/TOAST storage once per nonoverlapping cohort.
WAL, other databases, backups, and every other use of the volume belong in
`unmodeled_used_bytes`. Object-body storage on a separate backend needs a separate
forecast. Convert version-count retention to a measured equivalent time horizon
before applying this time-based model. Neither catalog dead-tuple estimates nor a
small correctness fixture establish a production bytes-per-row distribution.

```bash
python3 scripts/artifact_read_capacity.py --input observations.json --output forecast.json
```

The output distinguishes steady retention from delayed-maintenance backlog,
reports required capacity at the supplied reserve, and retains negative headroom.
Exit 1 means the modeled backlog crosses that reserve; exit 2 means invalid input
or an existing output file. No database connection or configuration change occurs.
An operator must validate the observations before using the result for alerts or
rollout approval.

## Local evidence

Use unused output directories and owned PostgreSQL servers. Follow
[Running tests](../testing/running-tests.md#artifact-read-query-growth-checks).

```bash
python3 scripts/benchmark-artifact-reads.py --stage treatment --versions 16 18 \
  --with-key-usage-tests --output /tmp/opencode/artifact-read-treatment
python3 scripts/benchmark-artifact-reads.py --stage treatment --versions 16 18 \
  --snapshot-source --with-key-usage-tests --with-notifier-tests --workload-profile \
  --output /tmp/opencode/artifact-read-workload
```

Keep `evidence.json`, per-major test logs, numeric request measurements, and the
actual repository EXPLAIN documents together. Reject attribution if source hashes
changed or cleanup failed. Startup and compilation failures are not baseline
measurements. A source revision alone does not identify an uncommitted build.

With `--snapshot-source`, passing results apply to the hash-checked source copy
and pinned executables retained under the evidence directory. Review
`live_source_changes_since_capture` before attributing that result to the checkout.
Deploy from the tested bytes, or repeat validation for the intended deployment
source. Source capture excludes local credential files and build caches; treat the
retained test configuration and fixture sources as private evidence.

The opt-in profile reports populated native partition fan-out, the actual prepared
event-source plan, complete expired-cohort deletion alongside reads, audit/summary
work, and before/after capacity statistics. `workload-summary.json` includes the
numeric p50/p95 and plan-work summaries. It neither supplies the missing legacy
latency baseline nor certifies deployment capacity.

The source-kind tests predeclare batches of 64. One and 64 distinct origins must
have equal source-scope and graph-record query counts. Their handler ceilings are
25 statements for 64 and 40 for 65. These are query-growth gates, not latency
targets. Payload/ciphertext projection counters must stay zero for authorization.

For object-backed preview/download and reconnect recovery, run the existing
[runtime-log correctness suite](runtime-log-verification.md#run-the-correctness-suite)
against an owned PostgreSQL server and versioned RustFS. Preserve its test and
cleanup logs. The browser tests separately exercise reconciliation with a healthy
socket and no delivered notification, and the notifier tests reproduce broadcast
loss without removing the client subscription. Those boundaries are not a live
browser-to-notifier end-to-end certification.

## Operator evidence

Do not restart services, change pool/autovacuum/retention settings, or reset a
populated database to collect this evidence. Have the operator identify:

- Backend revision, relevant build/source hashes, web asset revision, affected CLI
  versions, and PostgreSQL major/minor. A mismatch blocks comparison.
- Sanitized artifact/execution IDs, caller permission sets and constraints,
  retained version/source counts, graph depths, and active reader/writer counts.
- PostgreSQL pool limits, memory/planner settings, retention horizons and backlog,
  native partition inventory including DEFAULT, and parent analyze timestamps.
- Table/index/TOAST bytes and estimated live/dead tuples. These statistics are not
  exact bloat measurements. Record available database-volume bytes separately.

Compare the same fixture, permissions, cache state, concurrency, and completed
request counts before and after. Record cold and warm samples separately, with
errors and cancellations. Collect endpoint p50/p95, browser first usable render,
request rate, database reads/spills, pool/lock waits, notifier authorization work,
audit activity, and retention/summary-maintenance pressure. Keep tokens, SQL
secret binds, artifact bodies, and decrypted values out of the report.

The local p95 target is a 70% reduction for the history-heavy execution-artifacts
case. No percentage claim is valid without a fixed-source comparable baseline.
Exact totals still evaluate all eligible candidates and may dominate a large
request. A failing gate needs a measured follow-up or explicit contract review,
not an estimated total labeled exact.

Use retained bytes and measured write rates to forecast each configured retention
horizon plus the observed maintenance backlog. Document the sampling interval,
row-size distribution, workload mix, index/TOAST overhead, and forecast assumptions.
Choose deployment disk-headroom alerts from that forecast. Do not reuse a
TimescaleDB compression ratio or extrapolate this small correctness fixture into
a production capacity guarantee.

## Deployment and rollback

Backend and web must ship together with the v1 paginated artifact contract. Update
CLI and external typed SDK consumers before enabling that contract for them. See
[Artifact reads](../api/artifact-reads.md) for response shapes and consumer notes.
Keep a matching previous backend/web/CLI set for rollback.

This optimization does not require an index migration or live maintenance change
without further evidence. Review write/WAL/storage costs and build locks before
approving an index. Review role, timeout, cancellation, and cadence behavior before
adding supervisor-owned ANALYZE. Never put ANALYZE or partition DDL on artifact
request paths.

Rollout is blocked until required local correctness/load gates and the operator's
matched-workload report pass. Metadata/body authorization changes, timestamped
event provenance, and optional totals need separate policy/contract approval.
