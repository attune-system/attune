# File custody for ordinary completion archives

`FileEvidenceRetentionRepository::native_storage` consumes A's authenticated,
keyless `storage_evidence` projection. For each typed `completion_sources()` child
it registers the original `execution_result/<actual child ID>/""` archived site
before deferred raw-site resolution. Each child's `original_contract()` goes
through the existing contract-proof graph, including referenced type revisions.
This lets file reservation and readmission work after raw child result rows and
their original ciphertext have been garbage-collected.

The aggregate `/children/N` archive field namespace does not replace original
producer pointers. There is one real NativeArchive, not a synthetic snapshot per
child. F traverses A's existing exact physical-parent inventory, including each
original child execution, admission and frozen owner. It does not look up a
future issuer or merge contributor manifests into grants. C continues to own
reader authorization and plaintext disclosure.

ResultView keeps its original Action or Workflow contract kind. For a JSON worker
result, `/data/token` is the envelope field while `/token` is the schema-local
origin. A supplies the official mapping. F does not infer either from strings or
decrypt the archive. `storage_evidence` authenticates the actual typed release
before F accepts the archive. Ordinary completion has a completion release and
no wait/result-view release; a ResultView has its own release, not a wait lock.

The child count participates in the existing global closure row bound. A's byte
reservation remains authoritative, including original fields/contracts and release
rows. F does not separately charge duplicate ciphertext. The native chain bound,
predecessor deduplication, staged receipt checks, graph gates, generation-qualified
cache identity and child-first pre-LIMIT reclamation are unchanged. There are no
new parent enums, F node kinds, source grants, or migrations.

## Prerequisites and authored tests

This focused delta requires A's chain through `76419175`, including ResultView
`feb9a8d4`, H's ordinary failure projection and current J input initialization.
The APIs were read from the staged A chain. No shared A/H source was copied over
the older F branch, and no cache/group/future completion kind is treated as an
ordinary workflow failure by this adapter.

`crates/common/tests/support/file_native_completion_cases.rs` adds two real
repository regression sources, wired into the existing F native custody module:

- A real two-child ordinary workflow is admitted with complete schemas and sealed
  original input before Running. Each child publishes its original encrypted
  JSON worker result and becomes Failed. A's actual ResultView and H's actual
  ordinary failure token create authentic receipts and distinct release rows.
- Both raw children and their raw result ciphertext are deleted before initial
  file reservation. The real F keeper reserves encrypted files with object and
  shared staging obligations, retains the archive and each child's actual
  admission/frozen execution identity, and readmits after raw GC.
- The negative variant rejects actual unreleased ResultView/completion snapshots
  and isolated corrupt-contract copies despite recomputed outer digests and
  matching typed releases. The real originals are never rewritten.
- File-owned snapshots and archived ciphertext cannot be removed. Object cleanup
  alone does not release staging custody. Exact object and staging settlement
  releases real pin counts, then the existing bounded child-first collector
  removes the archives and cascades their typed release records.

The Rust unit source also checks independent child contract-reference traversal
and exact original envelope-site coverage. The Python source guards run without
importing Rust or any runtime fixture. All Rust/PG/filesystem regression sources
remain uncompiled and unrun by the agent. Source checks and rustfmt are not
database, kernel or provider acceptance.

This delta does not change native setup, controller enrollment, rollback recovery,
existing journal bindings, deployment packaging, or frozen migration bytes.
