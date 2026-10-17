# Native legacy filesystem controller

This is the deployment-owned implementation behind the existing
`StorageLegacyController` mTLS client. It ships an executable Python controller
and a pinned Rust native writer entrypoint, not just signed observations or an
operator's settlement claim. It is disabled by default. No production topology
is enabled by adding these files.

## Supported boundary

The native provider supports one bounded, controller-owned filesystem generation
on Linux cgroup v2 and local ext4/XFS. Both its blob and SharedFile staging roots
must be newly created by the controller at initialization, private mode 0700, and owned by a dedicated,
non-root service UID. The service runs in the HOST PID namespace. Its persistent
journal is outside both data roots and must survive controller restarts. E must
own and enforce this topology, not merely label a container as its owner.

All admitted writes go through this controller and its centrally built,
root-owned, SHA256-pinned `legacy_native_writer` executable. The writer is actual
native code using the common filesystem BlobStore's generic random staging,
multipart and copy path, or an actual process-owned checked SharedFile staging
write. It is not a simulated provider or the protected atomic PUT protocol.
It does not create Attune admissions, file evidence rows or provenance. The
deployment writer principal supplies actual original operation/version/root-read
and native/worker/attempt IDs obtained from their owning APIs and repository
inspection. A later audit must match EVERY durable member byte-for-byte. Do not
manufacture legacy rows to satisfy this interface.

This provider does not adopt arbitrary historical directories, mount aliases,
other mounted writers, missing request journals, untracked processes, or cloud
requests. Existing unknown history outside this ownership envelope remains
quarantined. Replacing a UID, path or root with a matching string does not transfer
ownership. No S3/RustFS management adapter is claimed: standard S3 data APIs have
no accepted-request drain operation. GCS, network filesystems, injected backends
and storage with outside hardlinks remain unsupported.

The entire generation is irreversibly closed at its first successful audit
fence. New application writes need a separately provisioned generation. This
adapter does not silently redirect storage, export files or rotate deployments.
E owns that lifecycle and the application's storage wiring.

## Real barriers and recovery

1. The deployment writer authenticates by mutual TLS and a pinned certificate
   DER SHA256. Its configured native instance, worker instance and execution
   attempt must exactly match the submitted member. An API management certificate
   cannot submit writes, and a writer certificate cannot request settlement.
   Neither credential is a human, execution API or runtime SDK credential.
2. The local repository commits each accepted request with SQLite WAL and
   `synchronous=FULL` BEFORE spawning a child. Operation IDs, original member,
   mode and payload digest are immutable. A retry returns the existing admission
   without re-executing it. Even an accepted-before-exec crash is not replayed.
   There are at most 256 admitted operations and 512 KiB per write.
3. The controller verifies the executable hash and ownership, uses an empty child
   environment, and attaches it to the delegated writer cgroup BEFORE exec.
   `PR_SET_PDEATHSIG` plus the post-arm parent check closes the parent-death
   fork/attachment race. Payloads travel through sealed anonymous memfds, never
   argv, environment, journal, disk spool, receipts or logs. The daemon is
   single-threaded so fork attachment and admission/fence transitions stay serial.
4. Stop-and-settle requires a plan covering the entire durable generation. It
   commits an irreversible admission fence before touching processes, freezes the
   cgroup, positively observes `frozen 1`, invokes `cgroup.kill`, observes
   `populated 0`, and waits for owned child handles. It also rejects other live
   processes using the dedicated UID. The controller itself is outside the writer
   cgroup. Neither exit status nor a timeout is used as provider settlement.
5. The provider then requires successful `syncfs` on BOTH pinned roots. For this
   local-filesystem-only adapter, cgroup drain has ended all producer syscalls and
   filesystem drain settles their accepted local I/O. There is no detached remote
   multipart service being inferred from process exit. The frozen cgroup remains
   a kernel restart barrier even if an old supervisor reattaches a task there.
   The controller never unfreezes it. An external writer outside this barrier is
   a topology violation, not an implicitly covered process.
6. Full descriptor-relative enumeration rejects symlinks, special files, nested
   devices and hardlinks outside the enumerated roots. Internal multipart/copy
   hardlinks are accounted for by exact device/inode/link count. Unknown paths
   block completion. Stage filename shape only identifies protocol item kind;
   ownership comes from the empty-origin, complete journal and native exclusion.
   Traversal is bounded to 2048 filesystem entries and depth 16. Receipts and
   inventory have explicit byte bounds. All-version/delete-marker coverage is
   inherent to this local provider, not an ordinary cloud current-object listing.
7. A restart reopens the same durable journal, mount/device/inode identities,
   cgroup identity, host PID namespace, boot identity and writer binary. Every
   verify, page and deletion revalidates native exclusion, drains again and checks
   the complete physical inventory. A stored fence is never echoed without these
   operations. Losing the kernel freeze, replacing a root, reverting the ledger
   or changing the host boot identity retains quarantine. This version recovers
   daemon crashes on the same host boot; it does NOT certify a reboot or rollback
   using elapsed time. Keep the original ledger and kernel fence intact.
8. Exact deletion first commits the catalogued inode-qualified intent. It unlinks
   only that exact root/path/inode, fsyncs its parent, drains the provider and
   revalidates the entire generation before recording absence. After a lost
   response, that same intent plus the surviving native fence and full inventory
   explains the missing identity. A missing path without the intent denies.
   Known internal hardlink ctime/link-count changes require accounted exact
   sibling-link deletions. No HEAD absence, TTL, tombstone or broad prefix delete
   can advance this protocol.

The journal stores original member metadata and credential-free native facts,
never provider keys, TLS material, payloads or raw backend errors. Its state is
deployment-local, not an Attune SQL migration or an alternate application DB
access path. Existing immutable app audit receipts retain the challenge-bound
RS256 observations through the normal repository APIs.

## Entrypoints and packaging contract

E provisions the dedicated UID, host namespace binding, private root PARENTS,
private persistent journal directory, delegated cgroup leaf, central binary and
independent TLS/signing material. Do not mount the Docker socket into the API.
Read-only access and mount topology must not give another application writer
access to these owned roots. A privileged deployment administrator remains part
of the controller's trust boundary; ordinary API callers have no host authority.

Use `scripts/legacy_storage_controller/config.example.json` as an incomplete,
disabled template. The file, TLS files and signing key must be root-owned and not
group/world writable. Use group-read permissions for the dedicated service UID
where needed. The journal directory belongs to the service UID at mode 0700.
The executable is root-owned and not group/world writable. Config placeholders
are rejected, not defaulted. Set `scope.backend` to the API client's actual JCS
digest of `{"storage": <storage>, "artifacts_dir": <path>}`. Both sides must use
the exact same storage topology, not a claimed root from caller JSON.
Install all six controller Python modules root-owned and read-only too, including
`http_input.py`. Use
Python isolated mode so inherited `PYTHONPATH` and the current directory cannot
replace controller imports. The service needs no Linux capabilities and sets
no-new-privileges before launching the pinned, non-setuid writer.
Use a dedicated non-root primary GID and no unrelated supplementary groups.
Native UID/GID, root and kernel identities are bound into the durable generation.
TLS and RSA private keys must have no world permission bits and must belong to
the service's dedicated group. Other live non-root identities sharing that
credential group are rejected. Public certificates may remain publicly readable.

These commands are packaging entrypoints, NOT authorization to run them on a
developer host:

```sh
python3 -I scripts/legacy_storage_controller/main.py check --config /etc/attune/native-files.json
python3 -I scripts/legacy_storage_controller/main.py init --config /etc/attune/native-files.json --enable-controlled-acceptance
python3 -I scripts/legacy_storage_controller/main.py serve --config /etc/attune/native-files.json --enable-controlled-acceptance
```

`init` creates fresh root inodes and refuses any existing journal or root, even
an empty root. Empty directories may still have old open descriptors. `serve` refuses a missing
journal and never initializes one. A crash partway through provisioning keeps
that generation quarantined; it does not authorize adopting the created paths.
Both require the explicit flag AND
`enabled_for_controlled_acceptance: true`. Production configuration stays off.
Use the server's HTTPS root as `artifacts.legacy_audit_controller.endpoint`.
The four management paths and signed response schema are exactly those in
[legacy file settlement audits](legacy-file-settlement-audits.md).

The additional `POST /native-writers/{operation-uuid}` requires the separately
pinned deployment writer certificate. Its bounded JSON body contains:

- `member`, the full exact `LegacyAuditMember`, including all optional fields;
- `mode`, `blob` for an object key, `shared` for an exact staging path, or
  `blob_and_shared` when the SAME original member owns both. Do not null out or
  split its frozen destinations to make a fixture fit. The combined native
  writer opens the shared staging file before the real multipart request and
  keeps its descriptor until that request ends;
- `payload_base64`, at most 512 KiB decoded;
- `pause_after_first_chunk`, a boolean. True provides a deterministic owned
  acceptance barrier with a real multipart session or shared staging descriptor
  still open. It is not a settlement signal.

An HTTP 202 means durably accepted, NOT completed or settled. A repeated identical
operation never launches another writer. No client-supplied URL, executable,
root, certificate identity, stopped flag or provider proof is accepted.

Native failures return a finite diagnostic and retain quarantine. Lost replies
are resumable through the original audit, not a reason to release pins. HTTP
request logs, subprocess stderr and payload logging are disabled. Management
observations use independent RS256 signatures and fresh request/nonce bindings;
writer authentication cannot mint them.
Input parsing has a five-second aggregate monotonic deadline, not a refreshed
per-byte inactivity timeout. Headers are capped at 16 KiB, body at 1 MiB, and
ambiguous Content-Length, Transfer-Encoding and non-JSON requests are rejected.

## Validation status

The source/mock suite is independently runnable without a service or database:

```sh
cd scripts/legacy_storage_controller
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -p 'test_*source.py'
```

`runtime_tests.py` contains central-only real SQLite reopen/admission/fence/delete
intent tests. They are NOT native process/provider acceptance. Central must
compile `legacy_native_writer`, run the existing signed-client tests, and have E
run the owning native deployment with genuine original legacy operation members.
Both BlobStore multipart and SharedFile cases must positively pass actual stop,
join, cgroup freeze, syncfs, catalogue and exact cleanup. Required failure cases
include an untracked writer/root, partial plan, wrong certificate/attempt,
controller death before exec, death after fence commit, provider-drain failure,
inode replacement, outside hardlink, lost delete response, unfreeze and restart.

No native, provider, application DB, compiler, deployment or production acceptance
is established by source checks. Keep production gates disabled until those
owned central validations pass. Standard cloud unknown-history audits continue
to return manual quarantine rather than borrow this local provider's proof.

### Executable central live cases

`scripts/legacy_storage_controller/live_cases.py` is the actual live runner.
`run_case(binding, case, execute_owned_live=True, allow_mount_fault=...)` is E's
importable lifecycle step. It is not a collection of injected check booleans.
It starts and joins the real dedicated-UID daemon outside the writer leaf,
observes actual paused multipart and shared-file descriptors in that leaf,
uses the normal authenticated application audit API and the typed repository
inspector, and requires real kernel/provider observations and exact pin release.
It never executes application SQL, creates legacy rows, or fabricates provenance.

Use `live-binding.example.json` as a deliberately incomplete single-lane template.
All original IDs come from F's real
[producer setup adapter](native-original-producer-adapter.md), never caller member
JSON. Each case needs
its own freshly initialized generation, empty request journal, cgroup leaf named
`attune-native-<generation-uuid>`, native UID/GID and correctly configured owning
application API. E initializes the roots with the real controller BEFORE starting
that application's storage consumers or allocating its genuine fixture. The live
runner will not adopt an already-used generation or weaken init to accept one.
The pinned adapter creates one exact original combined controlled native operation. Its actual
repository audit plan must equal the original journal member, including nullable
fields; mismatches fail rather than being patched by the runner.

The run-owned root must contain E's private mode-0600 `.attune-native-owner.json`
with exactly `run_id` and `project`, matching the private binding. Data roots,
controller configuration and journal must be beneath that root. E supplies the
matching newly compiled `native_settlement_adapter` and its private producer
context. The live runner invokes real setup before submitting the writer and
real repository inspection after metadata GC and settlement. No caller member or
check boolean is accepted. The older `storage_probe_inspector` remains a separate
read-only observation adapter, not a producer setup replacement. G's independent
runtime-policy observer is unchanged.

For E's separately provisioned single lifecycle:

```sh
python3 -I /installed/controller/live_cases.py --binding /owned/native-live.private.json --execute-owned-live --case paused_combined
```

The full CLI runs all nine original real SQLite reopen tests plus four enrollment
recovery tests and all seven required live
cases when `--case` is omitted. E must provision every named lane in `lanes`:

- `paused_combined`: real first-chunk pause, kernel freeze/kill/empty/join, syncfs
  on both exact roots, app catalogue and cleanup through source-pin release.
- `same_boot_restart`: real daemon SIGKILL/wait, no premature app release,
  same-boot restart, accepted request not re-executed, then full app cleanup.
- `lost_delete_reply`: actual exact delete, receipt body discarded, app pins
  remain, daemon restarted, durable intent recovered and actual app audit finishes.
- `untracked_pid`: an owned extra same-UID process outside the writer leaf causes
  native denial and actual application quarantine with pins/operations retained.
- `mount_replacement`: actual owned bind-mount replacement causes native root
  identity denial and application quarantine. It requires the extra
  `--allow-owned-mount-fault` flag and E's separate PRIVATE mount namespace and
  private propagation. Only that exact owned bind mount is unmounted afterward.
- `external_hardlink`: a real same-filesystem hardlink outside both data roots
  causes ownership denial. The alias and source pins remain for E's owned teardown.
- `cert_role_attempts`: actual TLS requests reject an unregistered same-CA
  certificate, manager/writer role confusion, a wrong original attempt and a
  caller-supplied root, without changing the accepted request. Explicit application
  quarantine and pin retention are verified again after daemon cancellation.

The daemon's real PID, UID, boot and PID namespace are recorded, as are kernel
populated/frozen state, old-writer disappearance, successful syncfs on both pinned
root FDs and repository release/retention results. Readiness and recovery use
bounded monotonic predicates; timeouts never count as settlement.

Missing host namespace, local provider or delegated cgroup returns UNSUPPORTED
and nonzero exit. A missing explicit mount-fault permission cannot silently skip
that case. Any failed case stops acceptance. A single `--case` result always has
`acceptance_complete: false`; it is a lifecycle component, not full acceptance.
Even full local success reports `cloud_acceptance: false`. E retains all roots,
journals, quarantine and original metadata evidence for its own scoped teardown.
The runner kills/joins only its own Popen handles, never arbitrary PIDs.

The pure source suite parses this runner but NEVER imports or executes it. Agent
validation must remain limited to those source/mock checks. Central-only commands
above authorize no agent kernel, process, provider, database or service jobs.
