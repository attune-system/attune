# PostgreSQL native maintenance protocol evidence

Date: 2026-10-06. Baseline: `59c9a9a0d00fbc431cc01a5c512da243c86b6a6b`.
Worktree: `/mnt/wdc/attune-worktrees/remove-timescaledb`.
Runner: [`scripts/probe-postgresql-native-maintenance.py`](../../scripts/probe-postgresql-native-maintenance.py).

The conversion probes below are historical experiments. On 2026-10-07 the user
chose fresh-database installation for this pre-production change; populated
phase-1 conversion is no longer a delivery gate. Protocol evidence remains useful,
but current migrations require their own fresh-install validation.

## Findings before the production design freeze

The transaction protocols work on PostgreSQL 16.15 and 18.6. DEFAULT repair has
an important limit: a day cannot be drained through committed moves into a
detached table while preserving ordinary parent-query visibility. The tested
repair moves the entire day and attaches its partition in one transaction.
An oversized day remains in DEFAULT. It needs retry and backlog reporting,
not partially committed draining.

A row-count budget alone does not bound DEFAULT constraint validation. PostgreSQL
may scan the rest of DEFAULT even when the target day contains very few rows.
The production operation needs a transaction deadline as well as a row budget.
Timeout means rollback, with the original rows still queryable.

The current candidate is an append-only invalidation log. Writers insert one record
per distinct kind/hour per statement, without a shared-marker UPSERT. Builders lock
the source parent first, serialize on a per-kind state row, and use repeatable read.
They acknowledge only explicitly captured visible notification IDs. Lower IDs that
commit late survive, including IDs reserved before the builder's snapshot.

Both PostgreSQL versions pass the append-log correctness probes. The old UPSERT
experiments remain below as historical evidence. The append-log workload also has
strict 10 percent performance failures, recorded without rounding away a miss or
discarding outliers. The actual repository, 30/90-day, and production performance
gates remain open. Readers still need source-parent locks before their data snapshot.

## Rerun and evidence identity

The runner uses only Python's standard library and cached Docker images. It does
not install extensions or run application migrations. Each server gets a unique
container, labeled data volume, and ephemeral loopback host port. SQL runs through
persistent `psql` sessions inside that container.

```bash
python3 scripts/probe-postgresql-native-maintenance.py \
	--repeat 2 --brief --output /tmp/opencode/native-maintenance-evidence.json

python3 scripts/probe-postgresql-native-maintenance.py \
	--protocols-only --versions 16 18
```

The output parent must exist. The runner refuses to overwrite evidence. `--brief`
omits individual transaction timings from stdout, but the output file retains them.
Each expected rejection includes its SQLSTATE and diagnostic. Failed invocations
also write evidence and exit nonzero. Protocol assertions never retry. Only database
startup connectivity and readiness predicates poll.

The main cost run is `818ff9ac5be4`, started at `2026-10-06T19:21:56Z`.
It used two fresh servers per version and changed the protocol order on repetition
1. Complete local evidence is `/tmp/opencode/native-maintenance-statement-final.json`.
The final expanded conversion probes are in run `b529f41bb95f`, and the additional
correction/deletion probes are in run `567376622af6`. Their files are
`/tmp/opencode/native-maintenance-conversion-final.json` and
`/tmp/opencode/native-maintenance-corrections-final.json`.
The final combined protocol run is `3ed17ccdee5e`, with two repetitions on each
version, preserved in `/tmp/opencode/native-maintenance-protocol-complete.json`.
It includes explicit destination locking, empty-destination verification, and
transactional DEFAULT-move invalidation.

Both images report x86_64 Linux musl, GCC Alpine 15.2.0, and 64-bit PostgreSQL.
The images were already cached; the runner uses `--pull=never`.

| Image | Server | Image ID and repository digest |
| --- | --- | --- |
| `postgres:16-alpine` | 16.15 | `sha256:721873c34ceb9f8d8fc265984940dc982404c105f19ad51be9fdc5970a6080ea` |
| `postgres:18-alpine` | 18.6 | `sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873` |

Settings in both versions were `TimeZone=UTC`, `default_transaction_isolation=read
committed`, `fsync=on`, `synchronous_commit=on`, `max_connections=100`, and
`shared_buffers=16384` eight-kilobyte blocks, or 128 MiB. Storage was a Docker named
volume. CPU and memory limits were not imposed. These are small SQL fixtures,
not the pinned disk-backed phase-1 workload.

## DEFAULT repair and visibility

The fixture has a RANGE parent, a daily leaf, DEFAULT, a timestamp index, and a
composite primary key. DEFAULT initially contains three rows on 2026-01-02 and two
rows on 2026-01-04. All timestamp bounds in this UTC session are half-open.

Both versions produced the same observations:

| Probe | Observation |
| --- | --- |
| Attach an empty daily table while matching rows remain in DEFAULT | `23514`, updated DEFAULT partition constraint would be violated |
| Commit deletion from DEFAULT and insertion into detached staging | Parent count falls from 5 to 2; staging contains 3 |
| Raise after moving rows, before attachment | `P0001`; rollback restores all 5 parent-visible rows |
| Repair the whole three-row day, budget 3 | Attachment succeeds; blocked reader returns 5; concurrent writer subsequently routes into the new leaf; final parent count is 6 |
| Preserve materialization during physical repair | Existing hourly count remains 3; subsequent late writer makes raw count 4; one dirty marker forces raw fallback |
| Try a five-row day, budget 3 | Bounded `LIMIT 4` probe reports over budget; all 5 remain in DEFAULT and visible through the parent |
| Cancel after moving rows in an uncommitted transaction | `57014`; all 5 oversized-day rows remain visible; transaction-created staging table disappears |
| Concurrent detach with DEFAULT present | `55000`, cannot detach partitions concurrently when a default partition exists |

The positive repair follows this order:

1. Prepare an empty standalone table with matching indexes and a validated day
   CHECK. Preparing it does not remove any source row.
2. Begin a read-committed transaction and set bounded lock and statement waits.
3. Lock `ONLY` the source parent in `ACCESS EXCLUSIVE` mode, then DEFAULT and the
   prepared destination in a stable order.
4. Check target-day size with a timestamp range against an indexed column and
   `LIMIT row_budget + 1`.
   If it exceeds the limit, finish without moving rows or attaching anything.
5. Move every target-day row using one `DELETE ... RETURNING` and `INSERT`.
6. Add a DEFAULT exclusion CHECK as `NOT VALID`, then validate it.
7. Attach the destination with the exact UTC day bounds. Drop the temporary DEFAULT
   CHECK after attachment, since the partition constraint now excludes that day.
8. Commit the physical move and its bounded summary invalidation together.

The standalone destination must be empty or an explicitly verified preparation
owned by this operation. A matching table name is not proof of ownership or bounds.
The production registry must verify catalog objects before adopting them.

The probe records `pg_locks`, including `ACCESS EXCLUSIVE` on the parent, DEFAULT,
and destination. A second connection's parent SELECT and a third connection's
INSERT both wait behind the repair transaction. The runner waits until
`pg_blocking_pids` identifies that exact blocker before proceeding. After commit,
the reader has no missing-row interval and the writer uses the attached leaf.
If writer scheduling changes, the reader can legitimately see 6 instead of 5.

PostgreSQL DEBUG output proves that both constraints avoid an *additional* attach
scan:

```text
partition constraint for table "repair_stage" is implied by existing constraints
updated partition constraint for default partition "raw_default" is implied by existing constraints
```

This does not make validation cheap. The preceding `VALIDATE CONSTRAINT` can scan
all remaining DEFAULT rows. Adding `NOT VALID` alone is insufficient evidence for
skipping validation. A previously validated exclusion can avoid this work, but it
cannot already exclude a day whose rows currently live in DEFAULT.

### Large DEFAULT backlog

The safe initial production result is `DeferredOverBudget`, not partial success.
Its diagnostic can report a lower bound of `row_budget + 1`, without counting the
whole backlog. For a small day inside a large DEFAULT, a validation timeout must
produce `DeferredBusy` or `DeferredDeadline`, roll back the entire move, and retain
raw visibility. Ordinary bounded expiry of old DEFAULT rows can still make progress.

Repeated retries do not guarantee that an unexpired oversized day eventually fits.
The supervisor must expose persistent backlog and missing coverage as operational
state. An installation may later use a separately budgeted write-pause conversion.
The normal maintenance path must not silently increase its budget or delete retained
rows to force attachment.

The probe does not establish that every imaginable partitioning design requires
one whole-day transaction. Subpartitioning DEFAULT or querying an explicit union
with staging would change the chosen structure or raw-query contract. Neither is
part of the tested daily-parent protocol.

## Lock order, wait limits, and expiry snapshots

With `lock_timeout='250ms'`, both a long read and a transaction containing an
uncommitted parent INSERT cause parent-lock acquisition to fail with `55P03`.
The error aborts the maintenance transaction and leaves raw data intact.

The inversion control locks DEFAULT first. The writer then owns a
`RowExclusiveLock` on the parent and waits for `RowExclusiveLock` on DEFAULT.
The maintenance transaction cannot upgrade to parent `ACCESS EXCLUSIVE` and times
out. Parent-first acquisition avoids this dependency cycle.

A second session fails to acquire the same advisory leader lock. Production must
reuse the existing supervisor lock key; the prototype's `760106` is fixture-local.

The main cost run measured these values. Server waits come from a
`clock_timestamp()` interval around the failing LOCK. Repair intervals start just
after parent acquisition and end with a timestamp just after COMMIT. They include
readiness coordination and a catalog inspection, and slightly overestimate the
actual lock hold.

| Version | Repetition | Reader-blocked server wait ms | Writer-blocked server wait ms | Repair hold upper bound ms |
| --- | ---: | ---: | ---: | ---: |
| 16.15 | 0 | 253.012 | 251.412 | 233.252 |
| 16.15 | 1 | 254.220 | 251.847 | 219.533 |
| 18.6 | 0 | 256.689 | 256.481 | 231.043 |
| 18.6 | 1 | 251.452 | 254.579 | 230.342 |

Docker-plus-psql wall times for these rejected LOCK commands were 316.770 to
328.573 ms. They include process startup and transport overhead. PostgreSQL timeouts
are cancellation targets, not hard real-time scheduling guarantees.

The later corrections run measured reader/writer waits of 252.749/252.084 ms on
PG16 and 253.096/252.508 ms on PG18. Repair upper bounds were 252.044 and 226.791 ms.
The final combined protocol run measured the following values, including physical
move invalidation and the destination lock:

| Version | Repetition | Reader wait ms | Writer wait ms | Repair hold upper bound ms |
| --- | ---: | ---: | ---: | ---: |
| 16.15 | 0 | 251.382 | 252.780 | 254.942 |
| 16.15 | 1 | 252.334 | 253.320 | 256.481 |
| 18.6 | 0 | 251.428 | 252.303 | 289.598 |
| 18.6 | 1 | 251.554 | 254.207 | 243.349 |

These tiny repairs fit the one-second target. They do not prove that a production
day, index build, or DEFAULT validation fits that target.

`statement_timeout='1s'` limits each statement, not the total transaction.
Production needs one bounded server-side maintenance step or a remaining-deadline
budget on each statement, plus prompt COMMIT or awaited ROLLBACK. It must include
callback waits and rollback time in observed parent-hold metrics. A client-side
timeout cannot leave an open transaction behind in a connection pool.

### Expiry during analytics

The expiry fixture contains three raw rows, one summary count of 3, and one coverage
record. Dropping the leaf and deleting both maintenance records uses one transaction
with parent-first `ACCESS EXCLUSIVE` acquisition.

The rejected reader begins repeatable read and reads the summary before locking
the source parent. Another connection expires the partition. The reader then sees
raw count 0 and summary count 3 in its old snapshot. PostgreSQL catalog changes and
table removal do not follow the same visibility rules as ordinary summary rows.

The accepted reader begins repeatable read, locks `ONLY` the source parent in
`ACCESS SHARE` mode, then takes its first data snapshot. Expiry times out with
`55P03`. The reader sees raw 3 and summary 3. After the reader commits, expiry
succeeds. New connections see raw 0, summary rows 0, and coverage rows 0.

The production ordering is source parents first, dirty markers second, then summary
and coverage modifications. Multi-source operations use a fixed source order.
Refreshers also need a source-parent read lock before locking their dirty marker.
Selecting candidates can happen outside that transaction; acquiring marker locks
before the source parent would reintroduce an expiry-versus-refresh inversion.

## Original UPSERT markers, bootstrap, and nullable groups

The main race uses an AFTER INSERT trigger that UPSERTs a marker keyed by kind and
UTC hour. Its conflict action updates a revision, so PostgreSQL takes a row lock.
Revision comparison is not required by the demonstrated acknowledgment protocol.

The two connections follow this exact sequence:

1. The refresher locks the existing marker and executes a source SELECT, preserving
   the resulting groups in a temporary table. The snapshot contains three rows.
2. A backdated writer inserts a fourth source row. Its trigger blocks at the marker
   UPSERT. The runner observes the refresher as its blocker.
3. The refresher replaces the summary from its frozen groups, records coverage,
   deletes the marker, and commits.
4. The writer completes and commits. Its UPSERT leaves a marker behind.

Both versions finish with summary 3, raw 4, dirty markers 1. A subsequent refresh
catches up. The `DO NOTHING` control instead lets the writer commit before the
refresher acknowledges the marker. That control finishes with summary 4, raw 5,
dirty markers 0. Serving that summary as clean would be incorrect.

An absent-marker bootstrap uses the same UPSERT before its source SELECT. A racing
writer blocks on that uncommitted marker, then leaves a new dirty marker after
bootstrap commits. The observed empty bootstrap ends with summary groups 0, raw
rows 1, and dirty markers 1. The next refresh includes the writer.

Read-committed refreshes must issue the source SELECT *after* marker acquisition,
as a separate statement with a fresh snapshot. The stale repeatable-read control
takes its snapshot before a writer updates the marker. Its subsequent
`SELECT ... FOR UPDATE` fails with `40001`. Retrying requires a new transaction,
not continuing with that snapshot.

The probes also establish:

- `UNIQUE NULLS NOT DISTINCT(bucket, ref, status)` makes `(NULL, NULL)` dimensions
  upsertable. A null ref stores count 7 while an empty-string ref separately stores
  count 2. No sentinel replaces nulls.
- Replacing all groups removes vanished groups. An empty completed hour has zero
  summary rows and one coverage row.
- An injected failed refresh rolls back summary deletion and leaves its marker.
- A partitioned-parent AFTER STATEMENT trigger with `REFERENCING NEW TABLE`
  deduplicates three inserted rows into two hour markers, each with revision 1.
- Its racing two-row writer leaves the refreshed hour dirty. The refreshed snapshot
  held 2 rows; raw subsequently has 4 and the new marker's revision is 1.
- UPDATE transition tables mark both the old and new hour for a cross-partition time
  correction. A dimension-only correction marks its existing hour.
- Parent DELETE transition tables mark the deleted hour. Rolling back deletion
  restores the source row and removes the transaction's dirty marker.
- Direct leaf INSERT does **not** run the parent's statement trigger. Its marker
  count is 0. Maintenance SQL directed at leaves needs explicit transactional
  invalidation; partition DROP never fires row DELETE triggers.

Deduplicated UPSERTs should use a stable kind/hour order. Transactions spanning
multiple sources must obey the same order across statements or retry a deadlock.
Statement-level deduplication reduces repeated updates, but a single dirty row
still serializes writers for the same hour until transaction end.

## Conversion, dependencies, and bounded row locations

The populated conversion fixture starts with two rows, IDs 1 and 2, an owned
BIGSERIAL sequence, an outbound foreign key, an insert-history trigger, a granted
view, and a SQL-standard `BEGIN ATOMIC` function bound to the source table. A
connection prepares a count query before conversion and remains connected.

Both versions successfully convert under a non-superuser owner role with schema
USAGE and CREATE. The transaction pauses writes with a table lock, renames the
heap, creates the partitioned replacement and covering leaves, and copies explicit
IDs. It transfers sequence ownership, replaces the view and bound function,
recreates the trigger after the copy, restores table grants, then drops the heap.

| Check | PG16 and PG18 result |
| --- | --- |
| Replacement failure before commit | Original parent OID and its two rows survive rollback |
| Copy-time business trigger calls | 0; the original two history entries remain |
| Sequence ownership dependency | `conv_event.id` on the replacement parent |
| First normal insert after conversion | ID 3; source and history counts both become 3 |
| Existing prepared connection | Count 2 after replacement; count 3 after the new insert |
| Rebound view and atomic SQL function | Count 3 |
| Observer table and view grants | Both reads succeed |
| Outbound foreign key | Invalid ref fails with `23503` |
| Role without schema CREATE | Startup DDL fails with `42501`; raw reads remain available |
| Role without parent ownership | ALTER fails with `42501`; raw reads remain available |
| ID-only `ON CONFLICT(id)` after composite-key conversion | `42P10`, no matching unique constraint |
| Explicit same ID on another day | Accepted; two records with ID 1, then rollback |

The dependency controls retain two rejected conversion designs. Renaming a heap
does not retarget its view: the view still counts 1 old row while the new same-named
table counts 0. Dropping the old sequence-owning heap before ownership transfer
fails with `2BP01`. Using CASCADE removes the replacement's default expression,
leaving zero entries in `pg_attrdef` for that table. The control rolls back.

Sequence allocation is not transactional. The failed foreign-key insert can
consume a sequence value, and rollback does not promise gapless IDs. Production
conversion must preserve sequence position without moving it backward, including
explicit historical IDs above its current value. These probes use an already
synchronized sequence; deployment-scale reseeding remains a conversion test.

The row-location fixture puts one row in each of two leaves. Both rows have the
same `ctid`. A deletion using `LIMIT 1` and `ctid` alone removes 2 rows, then rolls
back. The accepted statement selects `(tableoid, ctid) FOR UPDATE` and joins on
both values in the same statement. It deletes exactly 1 row.

A separate UTC horizon fixture creates January 1 plus seven future daily leaves
and DEFAULT, nine children total. January 1 and January 8 route to daily leaves;
January 9 and December 31 route to DEFAULT. In `America/New_York`, the two repeated
01:30 instants at autumn DST map to distinct UTC hours, 05:00 and 06:00.

## Measured toy costs and rejected performance claims

All samples below are from run `818ff9ac5be4`. They remain evidence even when they
miss a target. The runner does not filter samples or retry a slow measurement.

### Equal expiry cohorts

Each side contains exactly 10,000 rows. DELETE removes the entire ordinary cohort;
DROP removes the entire daily leaf containing the identical cohort. These server
intervals include SQL work but exclude COMMIT and vacuum. They are single samples
per fresh server, not p95 measurements or a production speedup claim.

| Version | Repetition | DELETE ms | DROP ms |
| --- | ---: | ---: | ---: |
| 16.15 | 0 | 5.424 | 3.623 |
| 16.15 | 1 | 5.577 | 2.764 |
| 18.6 | 0 | 5.581 | 3.038 |
| 18.6 | 1 | 6.176 | 3.043 |

### Four writers sharing one hour

Every sample has four persistent connections, each committing 20 transactions of
100 rows. Each sample inserts 8,000 rows with the same timestamp and alternating
null/empty refs. The table has a BIGSERIAL primary key. Baseline has no trigger,
`row` has one marker UPSERT per row, and `statement` has one UPSERT per distinct
hour per statement. All writers deliberately contend for the same marker.

A barrier releases the writers after connection setup. Wall throughput includes
the client loop and commits. Transaction p95 is the nearest-rank 95th percentile
of 80 intervals between SQL clock readings around INSERT and COMMIT. It includes
statement dispatch and commit, not just executor time. JSON retains all 80 values.
Sample order alternates; no other benchmark server runs concurrently.

| PG | Repetition | Sample | Tracking | Wall ms | Rows/sec | Transaction p95 ms |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| 16 | 0 | 0 | none | 454.520 | 17600.976 | 21.821 |
| 16 | 0 | 0 | row | 1384.616 | 5777.776 | 121.721 |
| 16 | 0 | 0 | statement | 944.976 | 8465.822 | 106.496 |
| 16 | 0 | 1 | statement | 946.400 | 8453.088 | 91.886 |
| 16 | 0 | 1 | row | 995.972 | 8032.353 | 89.133 |
| 16 | 0 | 1 | none | 637.817 | 12542.776 | 22.875 |
| 16 | 0 | 2 | none | 497.251 | 16088.441 | 23.191 |
| 16 | 0 | 2 | row | 1284.660 | 6227.330 | 123.166 |
| 16 | 0 | 2 | statement | 944.702 | 8468.279 | 88.107 |
| 16 | 1 | 0 | none | 452.983 | 17660.718 | 21.628 |
| 16 | 1 | 0 | row | 1274.986 | 6274.581 | 124.345 |
| 16 | 1 | 0 | statement | 934.972 | 8556.404 | 88.204 |
| 16 | 1 | 1 | statement | 1000.393 | 7996.859 | 121.890 |
| 16 | 1 | 1 | row | 1474.816 | 5424.405 | 134.197 |
| 16 | 1 | 1 | none | 669.760 | 11944.583 | 27.355 |
| 16 | 1 | 2 | none | 454.766 | 17591.466 | 21.740 |
| 16 | 1 | 2 | row | 1296.647 | 6169.761 | 123.599 |
| 16 | 1 | 2 | statement | 947.780 | 8440.776 | 111.437 |
| 18 | 0 | 0 | none | 453.420 | 17643.680 | 21.706 |
| 18 | 0 | 0 | row | 1494.087 | 5354.439 | 189.849 |
| 18 | 0 | 0 | statement | 1005.903 | 7953.050 | 145.715 |
| 18 | 0 | 1 | statement | 936.058 | 8546.477 | 142.575 |
| 18 | 0 | 1 | row | 1710.417 | 4677.223 | 302.056 |
| 18 | 0 | 1 | none | 456.298 | 17532.392 | 21.850 |
| 18 | 0 | 2 | none | 454.853 | 17588.093 | 21.761 |
| 18 | 0 | 2 | row | 1151.424 | 6947.921 | 122.531 |
| 18 | 0 | 2 | statement | 957.606 | 8354.169 | 99.297 |
| 18 | 1 | 0 | none | 454.597 | 17598.022 | 22.052 |
| 18 | 1 | 0 | row | 1152.415 | 6941.946 | 142.730 |
| 18 | 1 | 0 | statement | 948.489 | 8434.468 | 99.566 |
| 18 | 1 | 1 | statement | 950.246 | 8418.874 | 133.800 |
| 18 | 1 | 1 | row | 1545.493 | 5176.343 | 261.779 |
| 18 | 1 | 1 | none | 456.483 | 17525.280 | 22.883 |
| 18 | 1 | 2 | none | 478.285 | 16726.416 | 23.340 |
| 18 | 1 | 2 | row | 1295.763 | 6173.969 | 155.484 |
| 18 | 1 | 2 | statement | 1114.130 | 7180.491 | 166.291 |

Both tracking choices exceed 10 percent additional write time in every paired
sample. Statement deduplication preserves correctness and reduces repeated work,
but this evidence rejects a claim that it solves shared-hour contention. The actual
repository fixture must measure its real write transactions before choosing a
production trigger shape or another marker-key design.

### Raw and persisted count queries

Both queries describe the same 8,000 rows and two nullable groups. The runner
compares ordered JSON results to the raw oracle before timing. There are five
warm `EXPLAIN ANALYZE` samples per path, at concurrency 1. These tables do not have
the production summary indexes or repository filtering logic.

| Version | Repetition | Raw execution ms, all samples | Summary execution ms, all samples |
| --- | ---: | --- | --- |
| 16.15 | 0 | 2.010, 1.828, 1.846, 1.811, 1.802 | 0.052, 0.044, 0.051, 0.040, 0.083 |
| 16.15 | 1 | 1.833, 2.550, 1.812, 1.889, 2.213 | 0.043, 0.045, 0.042, 0.041, 0.046 |
| 18.6 | 0 | 1.702, 1.690, 1.771, 1.740, 1.727 | 0.064, 0.057, 0.059, 0.052, 0.087 |
| 18.6 | 1 | 1.721, 1.713, 1.768, 1.734, 1.956 | 0.057, 0.064, 0.049, 0.046, 0.051 |

The runner stores exact SQL. Raw groups records in `[2026-01-10 00:00 UTC,
2026-01-10 01:00 UTC)`, and summary selects the corresponding two persisted rows.
These results demonstrate a working measurement method, not the plan's sub-10-ms
repository p95 gate or its 500-ms raw-tail gate.

## Append-only invalidation protocol

This candidate replaces the single-row dirty-marker protocol while preserving the
same invalidation intent. The DEFAULT decision is unchanged: one atomic whole-day
repair within the row cap and deadline, otherwise retain queryable DEFAULT rows.
The committed detached-staging control remains rejected because its parent count
fell from 5 to 2 while three retained rows were outside the partition tree.

### Identity and rerun

The first append protocol run is `6998d67cac7a`, in
`/tmp/opencode/append-native-initial.json`. The ingestion run is `fcf2bec7d597`,
started at `2026-10-06T19:48:58Z`, in `/tmp/opencode/append-native-cost-first.json`.
It uses the same cached PG16.15 and PG18.6 images, storage, and durability settings
listed above. No benchmark servers overlap.

Run `9edcdd3a86a3` adds correction/deletion and rollback cases, with two repetitions
per version and reversed append-probe order. Run `80232889255f` repeats both the
original and append protocols twice per version, including the state-FK negative
control. Their complete files are `/tmp/opencode/append-native-protocol-final.json`
and `/tmp/opencode/append-native-combined-final.json`.

```bash
python3 scripts/probe-postgresql-native-maintenance.py \
	--append-only --summary --output /tmp/opencode/append-evidence.json

python3 scripts/probe-postgresql-native-maintenance.py \
	--protocols-only --repeat 2 --summary
```

The first command runs append correctness and all five ingestion profiles. The
second runs both protocol families, with no cost samples. `--summary` limits stdout
to protocol outcomes and paired gate results; an output file still contains all
timings, diagnostics, catalogs, and measurements. `passed` denotes protocol and
fixture assertions, not the separate `append_strict_10pct_pass_all_pairs` gate.

### Observed transaction races

Every final PG16/PG18 repetition produced these results. Counts below are summary,
raw, and pending notifications, in that order.

| Probe | Observed result |
| --- | --- |
| Lower ID inserted but uncommitted; upper ID commits first | Builder captures only `[2]`; late commit leaves `[1]`; counts `1, 2, 1` |
| Reserve ID 1 with `nextval` before insertion; ID 2 commits first | Builder captures `[2]`; later transactional source/notification insert with ID 1 survives; counts `1, 2, 1` |
| Refresh again after either late-commit race | Counts `2, 2, 0` |
| Rejected READ COMMITTED source freeze plus `id <= max_id` DELETE | Counts `1, 2, 0`; invalidation is lost |
| Bootstrap with a raw row but no notification | Captures `[]`; writer commits after snapshot; counts `1, 2, 1`, then `2, 2, 0` after catch-up |
| Three notifications, capture cap 1 | Recomputes all three source rows, deletes only ID 1; counts `3, 3, 2` |
| A second capped refresh | Counts `3, 3, 0`; replacement does not increment counts twice |
| Empty-hour bootstrap | Zero groups and one coverage row |
| Time/ref correction and DELETE | Rebuild removes all groups in the old hour; notification ID 3 for the new hour remains |
| Inject error after deleting captured IDs | `P0001`; rollback preserves pending IDs `[3,5]` |
| Two builders for one kind | Builder 2 waits on builder 1's state row; an independent source writer commits before builder 1 releases it |
| Waiting builder after first builder commits | `40001`; a fresh transaction retries and reaches `2, 2, 0` |
| Expiry while builder holds source-parent read lock | `55P03` with the 250-ms lock timeout; no partial expiry |
| Expiry after builder commits | Raw, summary, pending notifications, and coverage all become 0 atomically |
| Reader waits for expiry's parent lock before taking a snapshot | Reads raw 0, summary rows 0, and coverage rows 0 |
| Backdated writer after expiry | Routes through DEFAULT; counts `0, 1, 1` |
| Rejected notification-kind FK to the builder-state row | FK INSERT waits on the builder's `FOR UPDATE` lock |

Readiness comes from SQL-command completion and catalog predicates. For two builders,
the runner observes the second session in `pg_blocking_pids`, then waits for the
source writer's commit while the first builder still owns the state lock. The writer
has a one-second statement timeout and succeeds before the builder commits. For
expiry, the reader waits until the exact expirer owns its blocking parent lock.
No fixed sleep determines a race's ordering.

The low-ID reservation case uses a maintenance-directed leaf write with an explicit
parent read lock and an explicit invalidation insert in the same transaction.
Ordinary parent writes use the statement transition-table trigger. This tests a
notification whose sequence value was reserved before its row existed, rather than
relying only on a lower already-inserted transaction.

The max-ID regression specifically uses READ COMMITTED for the later DELETE.
A range DELETE using the very same repeatable-read snapshot would not see that late
row in this particular execution, but it still fails the explicit-ID acknowledgment
contract. A persisted max-ID cursor also cannot exclude lower IDs from later reads.
Sequence order never establishes commit order or notification coverage.

### Minimal DDL and production SQL recipe

The following is the event-volume instance of the proposed contract. Other summary
kinds use their existing source predicates and dimensions. These are proposed names,
not production migrations.

```sql
CREATE TABLE summary_invalidation (
	id BIGSERIAL PRIMARY KEY,
	kind TEXT NOT NULL CHECK (kind IN (
		'event', 'execution_status', 'execution_creation', 'worker_status'
	)),
	bucket TIMESTAMPTZ NOT NULL,
	CHECK (bucket = date_trunc('hour', bucket, 'UTC'))
);
CREATE INDEX summary_invalidation_bucket
	ON summary_invalidation (kind, bucket, id);

CREATE TABLE summary_builder_state (
	kind TEXT PRIMARY KEY,
	revision BIGINT NOT NULL DEFAULT 0
);
INSERT INTO summary_builder_state(kind)
	VALUES ('event'), ('execution_status'), ('execution_creation'), ('worker_status');

CREATE TABLE event_volume_summary (
	bucket TIMESTAMPTZ NOT NULL,
	trigger_ref TEXT,
	event_count BIGINT NOT NULL,
	UNIQUE NULLS NOT DISTINCT (bucket, trigger_ref)
);
CREATE TABLE summary_hour_coverage (
	kind TEXT NOT NULL,
	bucket TIMESTAMPTZ NOT NULL,
	refreshed TIMESTAMPTZ NOT NULL,
	PRIMARY KEY (kind, bucket)
);
```

There is no uniqueness constraint on notification kind/hour and no FK from
`summary_invalidation.kind` to the state row. The latter would take an FK key-share
lock and reintroduce writer blocking. A static kind CHECK or enum validates kinds
without reading or locking the builder state. Writers need INSERT and sequence
USAGE for the log; they do not read or update builder state.

An AFTER INSERT statement trigger on `event`, with `REFERENCING NEW TABLE AS
new_rows`, executes this plain insert and returns NULL:

```sql
INSERT INTO summary_invalidation(kind, bucket)
	SELECT 'event', date_trunc('hour', created, 'UTC')
	FROM new_rows
	GROUP BY date_trunc('hour', created, 'UTC');
```

UPDATE uses the distinct union of old and new hours. DELETE uses the distinct old
hours. History INSERT statements emit execution-creation hours for
`operation = 'INSERT'` and status hours for `'status' = ANY(changed_fields)`.
Direct leaf maintenance operations append the corresponding records explicitly in
their source transaction. Leaf DROP performs atomic summary/coverage/log removal
under its parent lock rather than relying on DELETE triggers.

The repository validates a complete retained UTC hour and a positive capture cap
before beginning the refresh. `$1` is the hour start and `$2` is the maximum number
of acknowledged notifications. Source-query work also has its own deadline.

```sql
BEGIN ISOLATION LEVEL REPEATABLE READ;
SET LOCAL lock_timeout = '250ms';
SET LOCAL statement_timeout = '1s';

-- No data SELECT occurs before these parent locks.
LOCK TABLE ONLY event IN ACCESS SHARE MODE;
SELECT revision FROM summary_builder_state WHERE kind = 'event' FOR UPDATE;
UPDATE summary_builder_state SET revision = revision + 1 WHERE kind = 'event';

CREATE TEMP TABLE captured_notification_ids ON COMMIT DROP AS
	SELECT id FROM summary_invalidation
	WHERE kind = 'event' AND bucket = $1
	ORDER BY id LIMIT $2;

DELETE FROM event_volume_summary WHERE bucket = $1;
INSERT INTO event_volume_summary(bucket, trigger_ref, event_count)
	SELECT $1, trigger_ref, count(*)::BIGINT FROM event
	WHERE created >= $1 AND created < $1 + interval '1 hour'
	GROUP BY trigger_ref;

INSERT INTO summary_hour_coverage(kind, bucket, refreshed)
	VALUES ('event', $1, clock_timestamp())
	ON CONFLICT (kind, bucket) DO UPDATE SET refreshed = excluded.refreshed;

DELETE FROM summary_invalidation n USING captured_notification_ids c
	WHERE n.id = c.id;
COMMIT;
```

Each parameterized SQL statement is sent separately through the same SQLx
transaction. The temporary ID set and raw aggregation share one repeatable-read
snapshot. The revision update gives a waiting stale builder an explicit conflict;
on `40001`, `40P01`, lock timeout, or cancellation, await rollback and retry the
whole operation within its budget using a fresh transaction. The probe permits
two-second builder waits to observe serialization; production waits remain bounded.

Bootstrap runs the same recipe with an empty captured set. It must not create a
fake notification or treat no existing notifications as proof that the hour is
already summarized. The dirty-hour read predicate is `EXISTS` on *all* currently
visible log records for kind/hour, never `id > last_acknowledged_id`. Analytics
pins coverage, notices, summaries, and raw records to one source-parent-locked
snapshot. Clean covered full hours use summaries; any pending record forces raw.

The state row serializes builders for a kind, including different hours. Writers
remain independent. Multiple source parents and state rows use a stable order.
Expiry takes its source-parent exclusive lock first, then affected kind state rows,
and removes source leaves and corresponding summary/coverage/notifications together.
The log's ID limit bounds acknowledgment, not the cost of recomputing a large hour.

Appending adds storage and deletion/vacuum work proportional to statements and
distinct hours. Repeated capped rebuilds can recompute the same full hour until its
notification backlog drains. Those costs need the supervisor's time/batch budgets
and backlog metrics. Fully expired hours cannot acquire new lasting coverage merely
because a late writer temporarily routes old raw data through DEFAULT.

### Four-writer ingestion results

Run `fcf2bec7d597` compares baseline and append tracking for three pairs per profile
on each version. Each sample has four persistent writers, each committing 100
transactions. Batch statements insert 100 rows, for 40,000 total source rows.
Single-row statements insert 400 total source rows. Every statement targets the
same UTC hour. Treatment produces 400 notifications except history batches, which
produce 800, one for each of the two relevant kinds per statement. Baseline produces
zero. Counts are asserted after every sample.

The event clone has all current event columns and ten indexes, including the JSONB
GIN and partial trace-tag index. Its JSON follows `evidence_payload(g,32)` in
`scripts/measure-timescaledb-removal.py`: request ID, host, 1,024-character records
string, and labels. Batch payloads average 1,160 stored bytes and 1,126.8 JSON-text
bytes. Single payloads are 1,160 stored and 1,125 text bytes. The history clone has
all seven history columns and all five indexes, including partial status/time and
GIN changed-fields indexes. JSON status/creation shapes match the existing fixture.
The definition sources are migrations `20250101000004_trigger_sensor_event_rule`,
`20250101000009_timescaledb_history`, and `20250101000016_source_trace_tags`.

Source JSON and row templates are precomputed outside the timed region and reused.
Batch history mixes 25 percent INSERT rows with 75 percent status changes; single
history uses a status UPDATE record. The clones omit application notification
triggers and outbound FKs and retain the baseline ordinary-table layout. The event
FK columns are null. These measurements match column/index/JSON shapes, not the
entire production write path or a populated 30/90-day database.

Writers start at a barrier. A server-side DO loop performs INSERT then COMMIT for
every transaction and records its elapsed server interval in a local array. This
avoids one client round trip per source transaction. Server intervals include
durable commit. Wall throughput includes the full four-writer loop. The p95 is
nearest-rank over 400 transaction intervals. Baseline/treatment order alternates,
and JSON retains every individual timing. Single-row p95 around 23 ms reflects
this Docker/storage environment and can mask smaller CPU overhead.

This prototype applies a strict check to **both** paired elapsed write time and
transaction p95: treatment must be at most 1.10 times baseline. Percent overhead is
not rounded before comparison. The current runner retains unrounded wall/p95 input
values too; this first run reported wall inputs to 0.001 ms. All pairs must pass to
mark the aggregate gate true.
It is false on both versions. The following table retains all 60 samples.

| PG | Profile | Pair | Tracking | Wall ms | Rows/sec | Transaction p95 ms |
| --- | --- | ---: | --- | ---: | ---: | ---: |
| 16 | toy-batch | 0 | none | 2274.511 | 17586.195 | 23.600 |
| 16 | toy-batch | 0 | append | 2487.449 | 16080.729 | 23.459 |
| 16 | toy-batch | 1 | append | 2417.864 | 16543.524 | 23.831 |
| 16 | toy-batch | 1 | none | 2317.349 | 17261.105 | 23.682 |
| 16 | toy-batch | 2 | none | 2453.299 | 16304.573 | 23.619 |
| 16 | toy-batch | 2 | append | 2263.380 | 17672.682 | 23.529 |
| 16 | event-batch | 0 | none | 3966.546 | 10084.340 | 84.244 |
| 16 | event-batch | 0 | append | 4255.360 | 9399.910 | 218.602 |
| 16 | event-batch | 1 | append | 3894.948 | 10269.713 | 93.277 |
| 16 | event-batch | 1 | none | 4119.317 | 9710.348 | 210.902 |
| 16 | event-batch | 2 | none | 3959.301 | 10102.794 | 173.924 |
| 16 | event-batch | 2 | append | 4567.799 | 8756.953 | 106.695 |
| 16 | history-batch | 0 | none | 3144.806 | 12719.384 | 52.052 |
| 16 | history-batch | 0 | append | 2928.446 | 13659.122 | 27.618 |
| 16 | history-batch | 1 | append | 2940.035 | 13605.282 | 56.684 |
| 16 | history-batch | 1 | none | 3172.432 | 12608.624 | 56.906 |
| 16 | history-batch | 2 | none | 2953.291 | 13544.213 | 46.234 |
| 16 | history-batch | 2 | append | 3137.210 | 12750.183 | 57.179 |
| 16 | event-single | 0 | none | 2290.547 | 174.631 | 22.743 |
| 16 | event-single | 0 | append | 2237.688 | 178.756 | 22.629 |
| 16 | event-single | 1 | append | 2284.974 | 175.057 | 22.661 |
| 16 | event-single | 1 | none | 2246.041 | 178.091 | 22.736 |
| 16 | event-single | 2 | none | 2414.365 | 165.675 | 22.847 |
| 16 | event-single | 2 | append | 2282.042 | 175.282 | 22.829 |
| 16 | history-single | 0 | none | 2343.254 | 170.703 | 22.607 |
| 16 | history-single | 0 | append | 2278.732 | 175.536 | 22.714 |
| 16 | history-single | 1 | append | 2243.579 | 178.287 | 22.669 |
| 16 | history-single | 1 | none | 2266.155 | 176.510 | 22.647 |
| 16 | history-single | 2 | none | 2342.889 | 170.729 | 22.641 |
| 16 | history-single | 2 | append | 2277.887 | 175.601 | 22.641 |
| 18 | toy-batch | 0 | none | 2281.268 | 17534.112 | 23.125 |
| 18 | toy-batch | 0 | append | 2514.576 | 15907.251 | 23.536 |
| 18 | toy-batch | 1 | append | 2315.895 | 17271.941 | 23.550 |
| 18 | toy-batch | 1 | none | 2349.122 | 17027.638 | 23.499 |
| 18 | toy-batch | 2 | none | 2486.929 | 16084.097 | 23.289 |
| 18 | toy-batch | 2 | append | 2282.153 | 17527.312 | 23.360 |
| 18 | event-batch | 0 | none | 3934.405 | 10166.722 | 195.136 |
| 18 | event-batch | 0 | append | 4153.177 | 9631.182 | 73.694 |
| 18 | event-batch | 1 | append | 3793.750 | 10543.657 | 80.619 |
| 18 | event-batch | 1 | none | 4058.976 | 9854.703 | 205.204 |
| 18 | event-batch | 2 | none | 3878.838 | 10312.367 | 115.586 |
| 18 | event-batch | 2 | append | 4372.357 | 9148.384 | 209.087 |
| 18 | history-batch | 0 | none | 2815.661 | 14206.253 | 47.821 |
| 18 | history-batch | 0 | append | 2757.536 | 14505.705 | 25.358 |
| 18 | history-batch | 1 | append | 2727.600 | 14664.909 | 35.794 |
| 18 | history-batch | 1 | none | 3385.792 | 11814.076 | 57.073 |
| 18 | history-batch | 2 | none | 2591.768 | 15433.483 | 25.694 |
| 18 | history-batch | 2 | append | 3243.753 | 12331.396 | 50.064 |
| 18 | event-single | 0 | none | 2269.417 | 176.257 | 23.030 |
| 18 | event-single | 0 | append | 2258.173 | 177.134 | 23.019 |
| 18 | event-single | 1 | append | 2258.033 | 177.145 | 22.861 |
| 18 | event-single | 1 | none | 2246.926 | 178.021 | 22.804 |
| 18 | event-single | 2 | none | 2261.290 | 176.890 | 23.041 |
| 18 | event-single | 2 | append | 2214.782 | 180.605 | 23.034 |
| 18 | history-single | 0 | none | 2354.099 | 169.916 | 23.078 |
| 18 | history-single | 0 | append | 2246.182 | 178.080 | 23.021 |
| 18 | history-single | 1 | append | 2292.613 | 174.473 | 23.312 |
| 18 | history-single | 1 | none | 2233.427 | 179.097 | 22.738 |
| 18 | history-single | 2 | none | 2277.163 | 175.657 | 22.738 |
| 18 | history-single | 2 | append | 2233.367 | 179.102 | 22.672 |

The failed pairs are explicit:

| PG | Profile | Pair | Wall overhead percent | p95 overhead percent |
| --- | --- | ---: | ---: | ---: |
| 16 | event-batch | 0 | 7.281 | 159.487 |
| 16 | event-batch | 2 | 15.369 | -38.654 |
| 16 | history-batch | 2 | 6.228 | 23.673 |
| 18 | toy-batch | 0 | 10.227 | 1.777 |
| 18 | event-batch | 2 | 12.723 | 80.893 |
| 18 | history-batch | 2 | 25.156 | 94.847 |

All single-row pairs passed this run. Toy batches passed all three PG16 pairs and
two PG18 pairs. Larger JSON/indexed batch samples show substantial variability,
including negative paired overhead. Those negative values are not evidence that
tracking speeds up ingestion. No failed sample was retried or excluded. The older
UPSERT and new append timings have different cohort sizes and measurement loops,
so this document does not claim a numerical speedup between them.

The append algorithm passes the requested race checks and removes shared-marker
transaction locking. It does not yet pass a universal 10 percent ingestion claim.
No production implementation or release acceptance is inferred from these samples.

### Append-run cleanup checks

All append success runs observe zero client sessions before container removal and
zero owned containers/volumes afterward. Run `0a6e9320ff6c` deliberately fails after
PG16 readiness while PG18 protocol run `bffe31e44b92` runs concurrently. Both clean
their own resources without cleanup errors. Their evidence files are
`/tmp/opencode/append-native-setup-failure.json` and
`/tmp/opencode/append-native-overlap.json`.

Run `41cfe25cb000` receives SIGTERM only after an external catalog query observes
expiry waiting behind `append_builder_one`, which holds the source-parent/state
locks. It exits 1 with `KeyboardInterrupt: signal 15`, no cleanup errors, and zero
owned resources. Evidence is `/tmp/opencode/append-native-cancel.json`.

The neighboring sentinel `attune-native-sentinel-append-832d21` survives both the
overlapping runs and cancellation. Its label remains
`local.attune.native-maintenance-probe=neighbor-append-832d21`. Its creator verifies
that label and removes it separately afterward. Final label-filtered inventories
are empty. Workspace Cargo check passes again in 0.51 seconds with no warnings.

## Minimal production contracts supported by these probes

The following interfaces are recommendations for the next implementation unit.
The prototypes do not create production schema or Rust APIs.

- `ManagedTable` has only event, execution history, and audit variants. Names,
  timestamp columns, partition names, and UTC bounds come from these typed values.
  Catalog descriptors include parent/leaf identity, bounds, DEFAULT status,
  ownership, and index compatibility. An explicit managed registry is checked
  against PostgreSQL catalogs before DDL.
- `SummaryKind` has execution status, execution creation, event volume, and worker
  status variants. Each maps to its exact source predicate, source-parent lock,
  dimensions, and summary table. Counts remain BIGINT and dimensions remain nullable.
- Four ordinary summary tables use explicit dimensional columns and
  `UNIQUE NULLS NOT DISTINCT`. A common append-only log has a BIGSERIAL primary key
  and `(kind, bucket, id)` index, with no unique kind/hour key and no state-row FK.
  Per-hour coverage records distinguish a completed empty hour from a missing
  materialization. Per-kind maintenance state stores bootstrap/backfill cursors
  and last successful work. Coverage cannot rely on a watermark alone.
- A maintenance budget carries row, bucket, operation, and elapsed-time limits,
  including lock acquisition and the parent-hold deadline. An outcome distinguishes
  `Applied`, `AlreadyPresent`, `DeferredOverBudget`, `DeferredBusy`,
  `DeferredDeadline`, and incompatible catalogs. Confirmed progress separates
  partitions dropped from rows deleted.
- `invalidate_source_hours_in(tx, source, hours)` appends deduplicated bounded hour records
  in the caller's transaction. It serves parent writes, ordinary history retention,
  leaf-targeted boundary deletes, and physical DEFAULT moves. A physical move
  invalidates a bucket without counting it as a business deletion or new event.
- `expire_source_range_in(tx, source, range)` removes summaries, coverage, and
  obsolete notification records for an expired day in the same transaction as leaf DROP.
  The partition repository acquires the parent first and calls this helper before
  commit. A late insert after commit appends a new notification through DEFAULT.
- `refresh_one_bucket(kind, hour, budget)` owns a short repeatable-read transaction:
  source-parent read lock, per-kind state lock, bounded capture of visible notification
  IDs, same-snapshot full-hour source SELECT, complete replacement, coverage
  publication, acknowledgment of only captured IDs, commit. Bootstrap uses this same
  method. Row-budget overflow or query timeout publishes neither false coverage
  nor partial groups.
- `begin_analytics_read(sources)` creates a repeatable-read transaction, locks
  source parents in stable order before its first data snapshot, and reads metadata,
  pending notification records, summaries, and raw records through that transaction. Clean covered
  full hours use summaries; boundaries, dirty hours, and uncovered hours use raw.

The `_in` methods are repository functions taking the caller's SQLx transaction,
not independent transactions or service-layer queries. The expiry method must
own its summary-removal callback so raw DROP cannot commit alone. Statement
transition tables are a proven candidate for deduplicating parent writes, but
direct leaf operations still need those explicit functions. Multi-source history
predicates, API authorization, pruning, migration dependencies in the actual
schema, and production read-plan coalescing need their owning integration tests.

## Failure preservation and resource ownership

The first invocation, `04a441b9dbca`, failed during initialization after a successful
Unix-socket query reached Docker's temporary init server. The final server was
not ready and the next query reported `the database system is shutting down`.
The runner now checks TCP readiness, which excludes that temporary server.
The failed sample remains in `/tmp/opencode/native-maintenance-initial.json` and
had zero owned resource leaks. The first successful protocol run is
`80e381a9e7eb`, preserved in `/tmp/opencode/native-maintenance-second.json`.

The expanded row-trigger cost run `b80410aec279` remains in
`/tmp/opencode/native-maintenance-final.json`. Its row-trigger transaction p95
ranged from 88.942 to 203.724 ms on PG16 and 113.988 to 168.028 ms on PG18. None
of those slow samples was removed when statement-level tracking was added.

Every server and named volume has label
`local.attune.native-maintenance-probe=<run-token>-<repetition>`. Cleanup verifies
that label before deletion. It closes persistent sessions and joins their output
threads before removing the container and its exact named data volume. Successful
runs observe zero client sessions before container removal, then zero owned
containers and volumes after removal. The shared Docker bridge is not removed.

The teardown checks also exercised these paths:

- Run `0c82445a7254` deliberately fails after PG16 readiness using
  `--fail-after-ready`. It exits 1 with zero owned container/volume leaks and no
  cleanup errors.
- It overlaps run `d76c00b6a43e`, a full PG18 protocol-only invocation. The latter
  passes and observes zero sessions before removal. Their loopback ports differ,
  41499 and 46087. The overlap run's repair upper bound is 298.861 ms.
- Run `a025b30ac5d7` receives SIGTERM only after an external catalog query observes
  a held `AccessShareLock` in the `long_read` session. It records
  `KeyboardInterrupt: signal 15`, exits 1, and leaves zero owned resources.
- An explicitly owned neighboring volume,
  `attune-native-sentinel-9a48d338`, carries the same label key with a different
  token. It survives the overlapping failure/success runs and cancellation. Its
  ownership label is checked afterward, then its creator removes it separately.

All final label-filtered container and volume inventories are empty. No janitor,
prune, fixed host port, existing database, or unowned resource cleanup was used.
The development work created only this evidence document and the runner.
`cargo check --all-targets --workspace` passed in 58.56 seconds with no warnings.

## PostgreSQL references

- [Partition maintenance and validated DEFAULT exclusion constraints](https://www.postgresql.org/docs/18/ddl-partitioning.html#DDL-PARTITIONING-DECLARATIVE-MAINTENANCE)
- [ATTACH and concurrent DETACH restrictions](https://www.postgresql.org/docs/18/sql-altertable.html)
- [Transaction isolation and snapshot semantics](https://www.postgresql.org/docs/18/transaction-iso.html)
- [Trigger transition tables](https://www.postgresql.org/docs/18/sql-createtrigger.html)
- [Unique constraints and null dimensions](https://www.postgresql.org/docs/16/ddl-constraints.html#DDL-CONSTRAINTS-UNIQUE-CONSTRAINTS)
