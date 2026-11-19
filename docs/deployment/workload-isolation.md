# Pack workload isolation

**Status: work in progress; s-23 remains open.** Kubernetes is the primary target;
Docker Compose is also required. The native adapters are not enabled defaults.
Current legacy Compose worker/sensor entrypoints retain their fail-closed
Bubblewrap prerequisite check and still fail under the tested ordinary Docker
procfs policy. A successful image build or offline chart render does not resolve
that deployment blocker.

## Execution boundary

The existing trusted worker and sensor loops retain scheduling, preparation,
DB/MQ access, encryption, custody and completion policy. They must delegate every
pack-controlled executable effect to an ordinary per-attempt/generation Pod or
container before the new path is activated. Physical separation of the manager
onto a remote host is not required. Manager, node/kernel, administrator and
launcher compromise are outside this threat model.

`common::isolated_process` is an in-process effects interface. The trusted loop
persists its intent and first-create decision before calling `prepare`. Only the
first durable submission permits `CreateOnce`; recovery uses `ReconcileOnly`.
A missing provider resource after an unknown create never permits another
create. Resource identity is the exact Docker daemon/container or Kubernetes
cluster/namespace/Pod UID. There is no caller-selected host path, mount policy,
container selector, HTTP dispatch credential or independent scheduler.

After a manager restart, the concrete `stop_retained` effect uses the original
public spec and resource identity without requiring a surviving stream/control
object, private invocation or current image admission. The caller must durably
close Run first. Stop returns only an observed exit; a Stop/exec reply or 404 is
not exit proof. It retains the transfer helper and private staging, and the
existing receipt policy still governs `remove_stopped`.

`crates/workload-backend` holds the two concrete adapters, safe ZIP/cache code
and public `attune-workload-runner`. The former standalone controller service,
frontend, session store and TLS proxy are retired. Their HTTP protocol is not a
requirement for the in-process path.

`Config.workload` contains the operator-selected immutable runtime image,
explicit platform, private cache root, memory/scratch limits and retained-tree
bounds. A shared public Runner ZIP pin and trusted package path are required for
both providers. The factory installs it and returns that exact pin for the
trusted producer's selected `/opt/attune/runner/linux-{platform}` input; it does
not extract code from an image or invent object identities. Its closed `backend`
selects Kubernetes (logical cluster, workload
namespace and immutable public helper image) or Docker (fixed Unix socket,
named volume/network, original daemon/platform binding and public probe image).
The backend factory reads only the trusted manager's
in-cluster Kubernetes identity; it never infers a developer context. Docker
startup verifies the expected daemon and a positive owned named-volume sentinel
probe. Its separate public helper image supplies that fixed probe, so a native
payload image need not contain a shell. No HTTP enrollment, dispatch or session
credential is configured between these in-process components.

The action environment renderer preserves the owning loop's explicitly selected
`ATTUNE_API_TOKEN`; runtime defaults cannot mint or replace reserved context.
It rejects known manager database, broker, signing, encryption and control
variable names. Setup plans and runtime probes reject action API tokens. These
checks neither issue credentials nor establish secret origins: permission-set,
owner, source and current-attempt admission remain with the trusted owning loop.

New source-built agent exports and release-package images include the public
archive at `/usr/local/share/attune/workload-runner.zip`. It contains only the
once-built executable at `linux-amd64` or `linux-arm64`, selected from the
compiler target. Release bundles include the same ZIP as the corresponding
published archive asset. The operator still supplies its exact digest, size
and bounds in `helper`; the package file alone grants no launch authority.
This is a source packaging contract, not a claim that older published image
pins contain the archive or implement native execution.

The optional field is migration staging, not permission for activated managers
to execute locally when configuration is missing. An explicitly configured
worker constructs the adapter and routes its existing action loop through the
trusted native producer. Worker/agent startup selects `NativeEffectsRequired`;
missed local workload/setup/probe wrappers deny before filesystem/PATH lookup,
without invoking the legacy Bubblewrap prerequisite. The source now includes
native producers for runtime discovery, coordinated setup, actions, candidate
tests and managed sensors. This implementation does not establish deployment
readiness: owning provider, startup, recovery and full-stack acceptance are
required before activation. Compose and chart configuration require explicit
image/Runner pins and private cache/custody storage. No published image pin or
working default deployment is implied.

Configured native managers select `WorkloadSandbox::NativeEffectsRequired`
through the existing child-environment interface. Residual guarded local
workload/setup/install/probe calls fail before executable lookup or Bubblewrap
startup. This is a migration deny guard, not proof that every caller migrated;
raw unguarded command sinks still require inventory and removal before activation.
Environment clearing and secret-bearing parent protection remain independent.

Language-neutral `execution_config.lifecycle` recipes use the fixed native
helper and one attempt-private generation for discovery, preparation and final
invocation. They are never published into the reusable runtime cache. Shared
legacy setup/invocation planners continue rejecting recipes explicitly rather
than dropping their tools, working directories or launch metadata. The trusted
local recipe profile is not an isolated-backend fallback.

A fixed Runner gate starts before pack code. Separate bounded stdout/stderr
attach streams are established first. The trusted caller commits the existing
input one-shot fence, then writes a bounded private invocation header and the
original selected stdin bytes exactly once. The header is not Pod metadata, a
public object or a plaintext parameter digest. The helper consumes only the
header, clears its child environment and preserves the application's remaining
stdin, including empty EOF, without adding a newline. Uncertain writes are not
retried. Existing CLI executable verification and static invocation-name semantics
remain part of selected invocation planning.

Declared JSON, YAML, dotenv and parameter-file delivery retain one canonical
JSON transport from the trusted producer. The guest formats that input after
preparation and retains any private parameter file through the final child
join. Inline action source is selected from the immutable action/release
snapshot, not manager filesystem existence or parameter rendering. Direct and
TempFile semantics are preserved; TempFile source is writable only in its
private guest directory. Parameter files are created with mode `0400`. Secret-bearing
stdin requires the retained canonical origin frontier; secret paths cannot be
materialized into CLI arguments. Input-only consent does not grant API access.

Installed-command selection uses the fixed Runner `inspect-command-program`
effect inside an admitted image/platform probe with the defining Pack and the
same public Runner pin. It resolves only the selected guest/runtime PATH,
hashes the selected read-only executable without launching it, and returns at
most 16 KiB of `SelectedProgram` metadata. Probe invocations carry no action
parameters or action API token. The trusted owning loop must bind this evidence
to its source, attempt, image/platform, release, runtime configuration and
helper before constructing the existing private exec envelope. Inspection
output alone grants no launch authority; manager PATH/architecture discovery
is not a substitute.

The fixed Runner `inspect-runtime-tools` effect executes the existing
`RuntimeRecipe::discover` tool probes inside an ordinary Probe. Its definition
and explicit guest PATH are private invocation context. The helper constructs
only that definition's exact local admission and a minimal child environment;
it never reads ambient recipe-admission grants or a manager PATH. Both selected
executable and canonical identity must be read-only. Only the bounded existing
`VerifiedTool` map is returned. The trusted owner still checks captured operator
admission, defining release, image/platform and successful Probe exit before
accepting versions. Recipe preparation and launch are not run by this discovery
helper. Registration uses the source-fenced probe driver; action, sensor and
candidate producers separately select the admitted private-generation helper.
Candidate suites that need global Python preparation complete it in the same
attempt before their recipe, while preserving the recipe's own environment and
PATH. Neither preparation phase receives the final candidate API credential.

## Kubernetes effects

The trusted worker uses a namespace-scoped Pod lifecycle/attach/exec identity in
a dedicated workload namespace. It creates ordinary Pods with
`restartPolicy: Never`; the existing worker owns retries. There is no Job
controller, operator, CRD, external Docker dependency or privileged DinD.

Each Pod has fixed `payload` and `transfer` containers in separate PID
namespaces. Service-account automount and service-link environment injection are
disabled. Containers run as UID/GID 1000 with a read-only root filesystem,
`no_new_privs`, all capabilities dropped and RuntimeDefault seccomp. Host
PID/IPC/network namespaces, host paths, control sockets, Secret/projected-token
volumes and manager/custody volumes are absent. Admission-injected authority,
mounts or containers cause policy verification to fail.

A fixed init helper copies the exact public Runner into a read-only bootstrap
mount. The payload command contains only this gate, not private argv/env. The
credential-free transfer helper accepts fixed admitted ZIP/file effects through
trusted Kubernetes exec; it has no arbitrary command listener or platform
credential. Selected pack/runtime/Runner roots are per-Pod volumes mounted
read-only in the payload; candidate tests receive their own writable pack copy.
Setup alone receives a private writable runtime stage at the stable
`/opt/attune/runtime` destination. Scratch and artifact staging are private to
the attempt. The Kubernetes Pod/stage must be retained until publication
receipts permit owned cleanup.

The attach transport requires Kubernetes stream protocol v5: stdin EOF must not
close stdout/stderr. Missing terminal transport status is incomplete capture,
not successful EOF. Actual supported cluster/runtime versions are established
by the provider acceptance matrix, not by the Rust client's schema version.

## Docker effects

The Docker adapter runs inside the trusted worker, using its operator-configured
local Engine socket and fixed named cache volume. The socket and cache parent
never enter a workload. An owned sentinel/container startup probe must prove
that the manager and daemon see the same named volume/subpath. Daemon identity
and immutable native platform are Docker-specific evidence, not Kubernetes
fields.

Docker also accepts the canonical exact local `sha256:<config-ID>` form for
owned candidate images. The Engine must inspect that exact ID and platform on
the configured daemon; an unavailable local ID fails without a tag or registry
fallback. Kubernetes requires named manifest/index digest references and rejects
local Docker IDs. No published image is inferred from a local build.

Each workload has a private PID/IPC/mount namespace, normal private procfs,
read-only root filesystem, all capabilities dropped and no-new-privileges.
Only selected immutable input subpaths and the attempt's own stage/artifact
subpaths are mounted. Container inspection checks exact ownership and fixed
policy before effects. Exit does not imply custody, file publication or removal.

## Storage, capture and readiness

Guest inputs and runtime publication use canonical v1 ZIP only. Source pack
releases retain the existing `PackStorage::publish_release` runtime-v1 manifest
and exact pinned archive size/digest. The trusted manager verifies that primary
GNU TAR inventory against the retained manifest before streaming it into a ZIP;
there is no TAR reader in the payload/transfer helper and no offline CLI-format
fallback or publication rewrite. Compressed length,
digest, decompressed bytes, entry counts/implicit parents, CRCs, path traversal,
link graphs, hardlinks and special files are validated. Safe links remain
confined to the selected fixed root; image-external venv interpreter links need
`--copies`. Setup must create, archive, publish and execute real console scripts
and internal links successfully in the same admitted immutable image/platform.

API-backed pack caches now retain that exact authenticated primary archive with
the atomic release cache publication, as shared-volume publication already does.
The action producer reads only the digest-derived retained archive plus the
same immutable release record's manifest, selects the configured image/platform
and public Runner pin, and reuses existing attempt/intent evidence on recovery.
Missing archive or unsupported runtime/CLI/origin proof denies admission; there
is no reconstruction from an unpacked tree or local executable fallback.

Setup's fixed Runner copies the primary release's regular-file project into
`/opt/attune/runtime/project` before executing creation, dependency and hook
steps. It rejects aliases, special entries, reuse and copy-budget overflow,
rather than rewriting links. Steps run in this private writable copy; the
admitted Pack remains read-only. Generated project/lock files are archived with
the runtime. Interpreted actions and sensors use the published project and
Runtime working-root convention, while candidate tests retain their own mutable
Pack source and working directory. Stage working-root still means the runtime
mount root. Native entrypoints remain selected from Pack. Ruby, Go, R and Perl
install commands now propagate failure; Node skips only an absent manifest,
not failed copying/installing. Real toolchain/bootstrap coverage remains part
of activation acceptance; pure planner tests alone do not establish it.

The concrete adapters' `archive_stopped_stage` effect accepts Setup only and
requires exact resource ownership plus successful payload termination, including
no OOM. Kubernetes reads a fixed retained ZIP from the credential-free transfer
container. Docker first observes exact Wait and a positive nonforced Remove
acknowledgement, then persists a private, owned removal receipt binding the
original public spec and exact daemon/container identity. Only then can it
validate/read the retained host stage: an exited Docker container alone can be
restarted. Receipt replay survives manager restart without inferring removal
from 404 or timeout; invalid/missing evidence leaves publication incomplete.
Both validate the bounded ZIP locally and preserve its exact bytes on retry.
This file is transfer evidence, not a runtime-publication receipt. The owning
runtime workflow must commit validation/publication before removing the retained
Kubernetes Pod or private host stage. The final Docker `remove_stopped` call
reclaims only that intent's private byte trees and retained publication ZIP,
preserving the small policy/removal record for idempotent recovery. Reading or
archiving never takes this cleanup path. Published input/runtime caches and
manager encrypted custody are independent. A failed or uncertain local cleanup
remains an error; an unexpected reread of reclaimed staging is lost-staging,
never a selected missing file. Real Python venv/pip console-script and absolute
internal-link regressions exercise both concrete providers, including reuse
after builder removal; compiling those tests does not establish that they passed.

For action, sensor and candidate-test file outputs, the concrete
`read_stopped_artifact` effect accepts only the owning loop's validated relative
path, remaining byte bound and exact stopped resource. It checks the full
retained artifact tree for aggregate byte/entry limits, links and special files
before reading the selected file. An empty regular file returns a file; only
selected-path ENOENT after a valid root/tree returns no file. Lost staging,
invalid trees, denied provider calls and transport failures remain errors.
Kubernetes uses a fixed bounded helper read. Docker first requires its durable
exact removal evidence, then opens only the attempt's owned private artifact
subpath. The trusted owning workflow retains inventory,
disclosure, publication and cleanup decisions; reading bytes is not a receipt.

The staged action caller transfers these selected files through the existing
authenticated artifact publication API, including for shared-volume managers.
It holds a private bounded snapshot through upload and exact-size completion
acknowledgements. The API retains attempt/origin checks and commits version
metadata; the caller performs no unchecked metadata write afterward. Empty
files require a positive zero-byte receipt. Missing files, invalid staging and
uncertain publication currently retain the owning attempt and resource rather
than permitting terminal completion or cleanup. Automatic recovery of these
pending file cases and full provider/file acceptance remain activation work.

Before extracting a new cached input or allocating a new Docker private tree,
the cache checks available filesystem space under its publication lock using
the admitted input/stage/artifact bounds. Reuse does not create a new disk
reservation, and other writers can consume space after this observation. The
trusted owning loops remain responsible for admission and concurrency.

Archive/output bounds, free-space/admission/concurrency checks and owned cleanup
are not a universal live filesystem byte/inode quota. Named-volume exhaustion
remains a residual availability risk unless a tested filesystem quota
prerequisite is added. Network egress is not isolated: scoped API access and
dependency downloads remain reachable.

The existing trusted worker performs bounded separate capture, output
classification, encryption and persistent log custody. Its independent spool key,
source/attempt fences, tombstones and completion idempotency remain required.
Provider exit alone cannot mark logs complete. Restart, lost stdin/completion
ACK, spool failure and missing custody must preserve finalizing/incomplete
outcomes rather than replay effects or invent EOF.

Default activation requires all action/CLI, setup/dependency hooks, runtime
verification/probes, candidate tests, managed sensors and artifact/secret paths
to use the same boundary. No legacy fallback or feature disabling closes this
requirement. Native provider effects tests do not substitute for the full
existing-worker capture/custody/fenced-completion proof.

The disposable Kubernetes effects fixture is
`cargo test --locked -p attune-workload-backend --test kubernetes_provider -- --include-ignored --nocapture --test-threads=4`.
It requires explicit owned kubeconfig/context/namespace, immutable candidate
payload/helper images and platform via `ATTUNE_ISOLATION_TEST_*`; it never
selects a developer/default cluster. Missing prerequisites fail. CI provisions
only its own disposable cluster. Chart rollout/storage/RBAC, packaged-image
acceptance, the full worker vertical and final independent security review
remain required before readiness.

## Explicit packaged native configuration (activation gated)

The draft chart no longer assumes an external execution host, controller or
enrollment API. Trusted worker/sensor managers remain in cluster with their CP
configuration and private persistent custody. They use the fixed native Pod
adapter in a separate operator-created restricted-PSA namespace. Role grants are
limited to Pods (get/list/watch/create/delete/patch) and attach/exec (get/create).
Guests have no SA token, CP mounts, sockets or shared writable runtime. Manager
node selectors/tolerations/runtimeClass remain manager placement controls; the
current closed guest policy has no arbitrary scheduling overrides. Supply actual
candidate manager/helper images and the matching public Runner pin; no old release
pin or render-only digest establishes support. Per-slot private PVCs and Recreate
retain custody across restart. Add independent slots for capacity.

For Compose acceptance, `scripts/prepare-native-compose.py` materializes the
ordinary Compose configuration with explicit native Docker settings, retaining
all services, worker profiles, sensor labels, recipe admission and dependencies.
It makes no Docker call and does not change the default deployment. Provide a
JSON map with exactly `worker-shell`, `worker-python`, `worker-node`, `worker-full`,
`worker-toolchains`, `sensor`, and `sensor-secondary`. Each value contains:

- `workload`: the exact current `common::workload_config::WorkloadConfig`, with
  fixed `cache_root: /var/lib/attune/workloads`,
  `helper_archive: /opt/attune/public/workload-runner.zip`, and Docker backend
  `socket: /var/run/docker.sock`. Image and helper image pins, platform, limits,
  exact public Runner object pin, expected daemon/native binding, dedicated
  cache volume and ordinary network are mandatory operator inputs.
- `runner_source`: absolute path to the real public Runner ZIP from the same
  candidate's existing agent/release packaging. Its bytes must match the pin;
  no second helper build or invented object identity is supplied by this script.
- `custody_volume`: a distinct persistent manager-private named volume.

The trusted operator configuration is native admission. `binding.engine_id` is
its stable supplied identity; it is not an enrollment row in the E2E database.
The native adapter compares the supplied daemon identity and platform with the
actual endpoint. Replacing a daemon requires explicit operator rebinding. Do not
allocate an ID from observations, insert legacy engine rows, or use the retired
controller enrollment path to manufacture authority.

Provision each cache/custody volume explicitly on the admitted daemon, owned by
UID/GID 1000 with mode 0700. Do not initialize over existing custody or rotate a
lost custody key. Networks and volumes are external, named inputs; no implicit
project-prefix inference binds a different volume. The manager alone mounts the
socket and private cache root. The production adapter verifies daemon/platform,
image and cache-volume binding before launching any guest, and mounts only
selected immutable inputs plus attempt-private staging in guests. These bounds
are not a hard live disk quota.

```bash
python3 scripts/prepare-native-compose.py --profiles /absolute/operator-profiles.json \
  --socket-gid 999 --output /absolute/new-native-acceptance
# Only after the coordinator grants the full integration lane:
docker compose --project-directory "$PWD" \
  -f /absolute/new-native-acceptance/compose.yaml --profile toolchains config
```

Use the actual socket GID, never the example number as an admission fact. Static
preparation verifies source archive bytes and preserves configured coverage; it
is not daemon, worker/PG custody, restart, runtime/probe, lifecycle, or smoke
acceptance. Native managers must keep denying any residual local launch. Do not
activate production defaults until every consumer and packaged profile passes.
The owned full-stack runner accepts the same profiles explicitly:

```bash
# Actual operator inputs and all stack images must already be prepared.
# Run only after the coordinator grants the full integration lane.
ATTUNE_E2E_RUN_ID="$OWNED_RUN_ID" \
ATTUNE_E2E_DOCKER_SOCKET="$OWNED_DAEMON_SOCKET" \
  bash scripts/run-integration-tests.sh \
  --native-profiles "$OPERATOR_PROFILES_JSON" --runtime-lifecycle --tier 1 \
  tests/e2e/api/test_sse_execution_stream.py tests/e2e/api/test_runtime_lifecycle.py -x
```

`ATTUNE_E2E_DOCKER_SOCKET` is an absolute, existing Unix socket, also used as the
manager's socket bind source. The runner sets the explicit `DOCKER_HOST` for all
child diagnostics/probes and clears inherited context/TLS selection. It verifies
the expected daemon/platform, existing pinned runtime/helper images, fourteen
distinct ordinary local volumes, and unused bridge networks. Each external
volume/network must carry `io.attune.test.owner=$ATTUNE_E2E_RUN_ID`; volume mount
users, shared writable volumes, observation errors and different ownership deny
startup. This is read-only verification, not provisioning or enrollment. The
prepared volume permissions above remain an operator prerequisite, and native
startup's cache binding probe remains authoritative.

Native mode uses prebuilt stack images (`--no-build --pull never` on managed
Compose startup/run calls). It starts all seven managers and waits for their
exact configured names to register. Generated Compose is the base; the existing
E2E name/port/vhost resets and selected lifecycle fixture admission follow it.
The toolchain worker gets its own E2E name/vhost reset. The API joins the owned
guest bridges so explicitly authorized guest API clients retain connectivity;
the database and MQ do not join those bridges. Worker capabilities, sensor
placement labels, selected tests, marker exclusions and existing deadlines stay
in force. Native profiles currently require fresh startup; standalone services
need their own native admission before they can be combined with this mode.
The ordinary runner path and standalone coverage remain available separately.

Compose owns its project resources. Native teardown stops/removes the Compose
containers and networks but retains project volumes, including database/object
recovery evidence. External native volumes and networks, plus the private
generated configurations/manifest under
`.scratch/native-e2e-<run-id>.*`, are deliberately retained even after Compose
teardown. Inspect the retained configuration for their exact names. Do not infer
native writer settlement from Compose teardown, empty listings, 404s or a test
exit code. A receipt-aware owner must first establish exact stop/removal and
custody completion before removing those volumes/inputs; this runner does not
provide that proof or rotate retained keys. Failed teardown also retains lifecycle
fixture binds. `--no-teardown` keeps those same inputs for recovery.

Selecting native mode in the checked-in full-stack CI call still requires actual
operator profiles, Runner object pins/archive bytes, prebuilt images, and
prepared run-owned endpoint/volumes/networks. The current CI call is unchanged
and continues selecting the legacy startup path. The repository supplies no genuine operator profile
for automatic CI creation; it does not replace that input with invented pins or
binding IDs. Pure mocked runner tests are wired into script validation. They
prove composition and failure handling, never native provider acceptance. The
historical b98 smoke failure was legacy sandbox startup failure; this integration
must pass the real full-stack lane before it can be called fixed or activated.

The existing `init-agent` copies the once-built public Runner ZIP alongside its
binaries; it does not mint an object pin or assert image compatibility.

The Docker owning-worker target is
`crates/worker/tests/isolated_docker_action_provider_tests.rs`. Its four native,
native-CLI, Command and interpreted cases construct the production configured
Docker backend and call the shared `tests/support/owning_worker.rs` domain
fixture used by Kubernetes. The shared support module retains the real preparer, repository,
capture/custody and restart assertions; it adds no alternate execution path. The existing Docker publication CI
lane builds the owning test executable once, alongside its once-built
Echo/Runner, and passes it to `scripts/test-launch-controller-container.py
--owning-worker-tests`. The driver reuses its same owned candidate images,
cache volume, socket and network, with the job's disposable PostgreSQL server.
Only the trusted manager receives the DB URL. Public candidate migrations and
`config.test.yaml` are packaged for the real TestDatabase helper; no source tree
is mounted into guests. Native profile tests require their explicit
`ATTUNE_ISOLATION_TEST_*` inputs; they never select a developer/default engine
or cluster. Authoring and static validation of this lane are not a passing
Docker/PG owning-worker result.
