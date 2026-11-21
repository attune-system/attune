# Language-neutral runtime lifecycle

Status: initial trusted-profile implementation validated locally; broader milestones
and required CI remain outstanding. Broader security
requirements remain implementation gates, not claims about current isolation. No deployment,
migration, database reset, or runtime admission is authorized by this document.

## Objective and scope

Support Bun, Python environments prepared with uv, optional PyPy, and toolchains
such as Deno, Gradle/JVM applications, and GraalVM Native Image without adding
language-specific execution implementations to Attune. A new runtime or build
toolchain should require pack content and provisioned worker tools,
without changes to Rust discovery lists, installers, invocation flags, or sensor
execution code.

Attune owns process lifecycle, authorization, environment publication, credential
delivery, resource policy, and audit. Pack-owned runtime definitions supply tool
discovery, preparation, and invocation instructions within operator policy.
Runtime definitions and their helper programs are executable content, even when
represented as YAML. Generic execution does not imply safe execution of untrusted
definitions.

Initial delivery targets trusted, operator-admitted definitions and dependencies.
Support for mutually hostile packs is a separate gated security milestone.
Do not silently broaden the existing deployment trust model during this refactor.

## Current implementation and governing contracts

- [Core runtime definitions](../../packs/core/runtimes/README.md) already describe
  interpreters, verification, environment creation, installation, and environment
  variables. [Python](../../packs/core/runtimes/python.yaml) and
  [Node.js](../../packs/core/runtimes/nodejs.yaml) are the initial examples.
- [Agent discovery](../../crates/common/src/agent_runtime_detection.rs) has a fixed
  interpreter candidate list. [Dynamic registration](../../crates/worker/src/dynamic_runtime.rs)
  also contains extension defaults. These are language-specific assumptions to remove.
- [ProcessRuntime](../../crates/worker/src/runtime/process.rs) executes configured
  setup commands and hashes one dependency manifest. [Environment coordination](../../crates/worker/src/env_setup.rs)
  also computes manifest checksums. [Sensor setup](../../crates/sensor/src/sensor_manager.rs)
  contains separate preparation logic. The new module must unify their behavior.
- [Runtime models](../../crates/common/src/models.rs) currently expose one file
  extension and dependency manifest. [Inline invocation](../../crates/worker/src/runtime/process_executor.rs)
  assumes `-c` for direct code. Explicit runtime selection already takes precedence
  over extension matching.
- [Child-process environments](../deployment/child-process-environments.md)
  govern inheritance, caller overrides, protected Attune context, and setup tokens.
- [Workload isolation](../deployment/workload-isolation.md) describes the
  opt-in Bubblewrap profile for discovery, preparation, tests and final
  action/sensor processes. The profile shares the manager's network namespace.
  Managers still hold control-plane credentials.
- [Delegation and secret disclosure](../permissions/delegation-and-secret-disclosure.md)
  governs immutable executable selection, attempt-bound authority, input origins,
  output policy, and log custody. The runtime refactor must preserve these contracts.
- [Runtime cache](../../crates/common/src/runtime_cache.rs) and existing preparation
  coordination provide mechanisms to evolve, rather than duplicate.

Keep canonical pre-production formats at v1. Revise their contract in place; do
not add v2 formats, dual readers, or language-specific compatibility adapters.
Use repositories for persistence and preserve SQLx transaction/session ownership.
This plan does not require a database migration by assumption: assess persistence
and snapshot changes after finalizing the contract. Any migration follows
[PostgreSQL deployment policy](../deployment/postgresql-only.md).

## Proposed module and interface

Introduce one shared runtime lifecycle module used by workers, sensors, and
candidate test runners. Keep policy and credential handling in trusted callers
and enforcement code; recipes never construct security authority.

Conceptual interface, not final Rust signatures:

```text
discover(admitted_definition, pool_policy) -> verified_runtime_instances
prepare(build_instance, pinned_release_bindings, preparation_request) -> prepared_target
invoke(prepared_target, execution_instance, admitted_invocation_context) -> running_process
```

Contexts are typed, server-owned capabilities, not arbitrary maps supplied by a
pack. A running process supports bounded output collection, cancellation, timeout,
and awaited teardown. Internally separate policy enforcement, command execution,
filesystem ownership, and repository coordination where they vary. Callers use the
same external interface in tests and production.

A prepared target binds the selected entrypoint to its environment and retained
launch files. It can represent a source script, JAR/classpath, application directory,
or native executable. Preparation includes dependency setup and optional compilation;
invocation does not assume `interpreter <source>`. Packaging tools remain commands
inside definitions/helpers, not new execution implementations in Attune.

### Definition contract

Extend the canonical runtime definition with the following concepts. Field names
and exact serialization remain a phase-1 deliverable; existing examples are not
an approved new schema.

1. **Identity and discovery.** Stable owner-qualified runtime identity, aliases,
   source release/digest, required tool probes, executable lookup rules, expected
   exit status, bounded version extraction, and version constraints. Discovery
   returns resolved absolute executable identities. Multiple required tools retain
   separate versions: a dependency manager's version is not the interpreter's.
   PyPy's language version and implementation version must remain distinct.
   Declare the primary version used by action constraints and highest-version
   selection independently of auxiliary tool versions. Describe preparation and
   execution requirements separately, including vendor/features where material.
2. **Preparation inputs.** Explicit relative files or bounded patterns, including
   manifests, lockfiles, tool configuration, helper programs, local dependencies,
   source trees, entrypoint/build target selection, and workspace inputs that affect
   the result. Define missing optional versus
   required inputs, deterministic ordering, and symlink treatment. The immutable
   release digest covers all pack bytes even when the declared input set is narrow.
3. **Preparation steps.** Ordered executable/argument arrays with explicit working
   directory and environment templates. Required failures stop publication;
   optional steps must not hide required verification or security failures.
   Complex preparation can call a release-pinned pack helper. Avoid an unbounded
   shell-like configuration language or a Rust adapter for each package manager.
4. **Prepared outputs and invocation.** Declare retained files/directories, the
   launch executable and argument templates, working directory, source mapping,
   and execution requirements. Support both source and compiled targets. A trusted
   supervisor resolves the launch description from the admitted definition and
   validated output paths; helper-generated metadata is untrusted input, not
   permission to select arbitrary host executables, mounts, or credentials.
   Explicit runtime references are
   authoritative. Extension lists are authoring hints, not authority or implicit
   replacement of an explicitly selected runtime.
5. **Environment.** Explicit runtime application variables and exact permitted
   caller overrides, subject to existing protected-variable rules. Define tool
   configuration/cache roots within owned storage. Definitions may request
   capabilities; only operator policy can grant them.

Template substitution preserves argument boundaries. Action parameters arrive as
stdin JSON, never interpolated into shell commands. Runtime paths and caller data
must not be confused with shell source or command-line options. An explicit shell
step remains executable code with the same authority restrictions as any helper.

Inline actions materialize an owned temporary source file with a definition-owned
suffix, then use the same preparation/launch path as file-based targets. The source
identity and declared project-relative mapping must participate in preparation
identity; attempt-local inline builds stay private and are retained through use.
Preserve import/module lookup behavior and avoid embedding secrets in generated
source. Remove the implicit `-c` contract once consumers use this canonical path.

### Discovery and placement

Load authenticated, release-pinned definitions before probing. For connected
workers use the existing authenticated control path; operator-owned bootstrap can
provide a pinned local bundle before connectivity. Resolve the bootstrap ordering
explicitly so detection does not depend on having already detected a runtime.

An operator-managed worker-pool policy admits exact definition identities/digests
or a deliberately selected publisher policy. Merely publishing a definition does
not execute it everywhere. Pack install authority remains the existing scoped
deployment authority; admission to a privileged worker pool is a distinct decision.
Aliases use `normalize_runtime_name()` and do not replace another owner's identity.

Probes run as executable workloads with limits and no control-plane credentials.
Bound process count, parallelism, stdout/stderr, extraction size, and duration.
Capability records identify verified instance, definition, tool versions, platform,
and policy identity. Placement uses those records, not a recipe's unverified claim.
Revalidate at use when tools/images change; disappearance or identity mismatch
fails or triggers verified reselection within the captured constraints.

Preparation workers advertise verified build tools and resource/policy support;
execution workers advertise the target's required execution tools and platform.
An available compiler does not prove that a different worker can execute its
output. Track build host, target OS/architecture, ABI/libc, required shared libraries,
and execution tool/vendor/features as applicable. Native output is not assumed
static, cross-compiled, or portable. Initially build and run on one eligible worker;
cross-pool execution is unavailable until authenticated output distribution,
retention, compatibility validation, and path-relocation rules are implemented.

### Workload and runtime-provider release bindings

An action or sensor in pack A may select a runtime owned by pack B. Capture
separate immutable release bindings for the workload and the runtime provider,
including the provider's selected definition/version and helper artifacts. Snapshot
metadata alone does not retain or distribute B's executable files. When A and B
are the same release, storage may deduplicate bytes without losing either role.

Resolve `{pack_dir}` to A and a separate `{runtime_provider_dir}` to B. Helper paths
are relative to the pinned provider root and cannot resolve through a mutable pack
ref or caller-supplied path. Retrieve both through authenticated, integrity-checked
release distribution and mount only their selected inputs. Candidate releases need
the same binding contract before activation. Capturing B's content does not grant
A additional API rights or permission to replace B's definitions.

Retain both releases/artifacts while queued executions, retries, admitted sensor
generations, builds, or prepared-environment users reference them. Updating or
deleting B's authoring records must not remove retained bytes or silently switch
A's binding. Explicit security revocation follows its separately defined admission
policy and can deny use; it cannot silently substitute a different provider.

### Build workspace and prepared outputs

Keep workload/provider releases immutable. Tools that write alongside sources get
an owned writable working copy or enforced copy-on-write workspace derived from
the pinned inputs. Preserve project-relative paths and bound copying, symlinks,
workspace membership, and disk use. A helper receives distinct workload input,
provider input, build-workspace, scratch, and output roots; inputs remain read-only.
Avoid linking mutable workspace files back into release or shared-cache storage.

The definition declares which outputs and supporting resources survive preparation
and how the entrypoint maps to them. Validate paths, types, executable permissions,
link containment, and execution requirements before trusted publication. Publish
the target metadata and retained file identities atomically with the generation.
Private preparation credentials/config and disposable scratch are not published.
Preserve stable physical paths where outputs embed build paths; retain necessary
workspace subtrees instead of assuming every tool's output can be moved.

During invocation, published executable content is read-only in profiles requiring
it. Tools needing writable caches, generated code, or task working files use owned
per-invocation scratch/copies within policy; reusable executable content stays
protected. A Gradle task executed as the action is an ordinary invocation with its
own scratch and result/artifact handling, not an implicit mutation of a shared
prepared application. In either mode the action must preserve stdin JSON, output
classification, and artifact delivery, using a pinned launcher/helper if needed.

A build target/entrypoint is captured before preparation and is part of reuse
identity. Prewarming needs declared finite targets or a known complete input graph;
do not infer all possible dynamic imports or task names. Invocation-time parameters,
secret inputs, and scoped action tokens are unavailable to shared preparation.
Initial lifecycle-managed attempt-private builds accept only pinned source and
admitted nonsecret build configuration, including inline source; they receive no
rendered action parameters, sensor secrets, or action/sensor API token. Compilation
depending on invocation inputs is unavailable as a preparation feature until its
attempt-bound authority and secrecy contract is specified and validated.

That later phase must bind input-origin recording and secret delivery to the exact
execution attempt, protect diagnostics/intermediates/binaries/workspace copies as
private material, fence retries and cancellation, and own teardown/disposal. Its
outputs cannot enter shared preparation caches. Retained intermediate artifacts
need explicit authorized storage/disclosure and expiry. This protection does not
reclassify final action outputs: their admitted schema/channel policy and recorded
origins remain authoritative. An admitted action can itself invoke a compiler under
ordinary invocation authority; doing so does not authorize promotion of its files
into a reusable prepared target. Never put secrets into public cache keys.

### Preparation, reuse, and cleanup

Compute a prepared-target identity from the immutable pack release, canonical recipe
and helper digests, both workload/provider release bindings, declared dependency
inputs, source/entrypoint/build target selection, resolved build and execution tool
identities/requirements, target platform, worker image identity, effective build
configuration revision, preparation policy, and trust scope. Credentials and
secret values never enter public digests, marker files, or audit details.

Capture the complete effective preparation configuration, including selected
operator passthrough/environment, explicit overrides, referenced config files,
registry/index selection, and immutable startup configuration. Every exposed setting
must be prohibited, bound to an immutable configuration revision, or explicitly
classified as irrelevant to reuse by operator-controlled policy. The default for
an unclassified or mutable preparation input is no reusable preparation result.
Use an opaque nonsecret revision for sensitive configuration; changes invalidate
reuse without hashing secret values into public keys. A worker restart is not proof
that two configurations are equivalent, and a common image is not a config pin.

Credential-dependent acquisition additionally binds the authorized registry/source
scope and credential-policy revision to its trust partition. Credential rotation
or a change in effective access invalidates reuse conservatively through those
opaque revisions. Unversioned credential/configuration state disables reuse; this
does not relax the separate private-registry delivery gate. A referenced file must
be an immutable pinned input, not a mutable path asserted to have a stable revision.

Reuse existing environment coordination. Each claim attempt receives a fresh,
generation-specific physical directory at its permanent usable path; concurrent
readers cannot use it until trusted publication. Do not assume arbitrary
environments are relocatable:
virtualenv shebangs and symlinks may encode absolute paths. A trusted supervisor
publishes a mapping from logical environment identity to that generation only
after required verification, permissions, and ownership checks. Child-created
marker files alone cannot authorize reuse.

All compiler, package-manager, wrapper, and daemon descendants belong to the build
attempt. Do not adopt a shared daemon from another attempt/trust scope. Disabling
daemon reuse is defense in depth; preparation must still supervise helper processes
and any single-use daemons. Before publication, await their exit and terminate/join
remaining owned descendants. Uncertain writers quarantine the generation rather
than publishing it while they may still mutate executable output.

Fence stale lease owners and publication attempts. A database fence cannot stop
filesystem writes from an unreachable manager or surviving descendant. Lease
expiry therefore never authorizes reuse of the same physical directory. Quarantine
uncertain generations with their ownership records; a replacement uses a fresh
generation. A protected profile must also prevent an old builder from reaching
new generation paths, using enforced mount/identity isolation. Generation naming
alone does not isolate malicious same-UID processes in trusted mode.

On cancellation, claim loss, failure, or manager restart, stop/join reachable
writers. Cleanup of uncertain generations waits for acknowledged teardown or
verified external host fencing; otherwise quarantine remains visible as operational
debt. Publication atomically rejects stale owners. On a race, readers pin the exact
published generation, not an authoring ref or mutable directory alias.
In-use environments remain pinned through their users; deletion/rebuild waits for
those users or creates a separate environment. Shared-volume/host topology and
filesystem visibility must be part of coordination identity and tests.

Published packs and environments are mounted read-only for final workloads when
the selected profile requires it. Trusted mode retains its documented writable
environment risk where that mechanism is unavailable. The hostile-pack profile
prevents cross-trust build/download cache modification, including via hardlinks or
symlinks; either trusted profile supports only the
admitted shared trust scope. A content hash identifies inputs, not benign output.
Cleanup acts only on proven owned roots with path-race defenses, never age alone.

### Tool provisioning and ecosystem coverage

Operators provision version-pinned worker tools/images. Recipes do not gain root,
modify the host toolchain, or install arbitrary interpreters during final invocation.
An optional future acquisition mechanism would be separately operator-authorized,
integrity-verified, and reflected in discovery and environment identity.

- **Bun:** a pack definition probes Bun, prepares package dependencies outside the
  read-only pack, and executes JS/TS files. Cover module lookup, lockfile presence
  and frozen installs, local/workspace dependencies, lifecycle scripts, config
  discovery, automatic `.env` loading, and runtime auto-install behavior. Type
  checking is an explicit authoring/test command, not a side effect of execution.
- **Python with uv:** Python remains the interpreter; uv is a required preparation
  tool. First cover requirements-based environments with an explicitly selected
  interpreter. Add locked `pyproject.toml`/`uv.lock` projects after defining manifest
  precedence, groups/extras, project installation, external environment targeting,
  local/workspace dependencies, and required lock validation. Disable automatic
  interpreter acquisition for the initial implementation.
- **PyPy:** an optional separate definition selects PyPy explicitly and can reuse
  uv preparation. Keep caches/capabilities distinct from CPython and test package
  compatibility and version reporting. Measure representative workloads before
  claiming a performance benefit.
- **Deno:** probe Deno and prepare declared entrypoint dependency graphs from
  `deno.json`/`deno.jsonc`, lockfiles, import maps, npm dependencies, and workspaces
  as applicable. Place vendored modules/caches in owned roots. Define frozen-lock
  and cached-only execution behavior, disabled permission prompts, and narrowly
  requested Deno permissions. Dynamic imports missing from preparation fail closed
  under no-acquisition execution policy. Test read-only published content with
  invocation-local writable cache needs; Deno flags supplement OS policy.
- **Gradle/JVM:** separately identify build JDK, Gradle distribution/wrapper, and
  application execution JDK. Include wrapper scripts/JAR/properties, build/settings
  scripts, plugins, version catalogs, lock/verification metadata, subprojects, and
  sources in build identity. Provision a pinned distribution or authorize verified
  wrapper/toolchain acquisition during preparation only. Build in the writable
  workspace with owned user-home/project caches. Retain the declared JAR, classpath,
  or application distribution and launch it directly when preparing an application.
  Also support a definition whose final action explicitly executes a Gradle task.
  Own daemons and descendants in both modes; no Gradle-specific Rust lifecycle.
- **GraalVM:** model JDK execution, a specific installed guest-language distribution,
  and Native Image builds as explicit definitions/tool requirements rather than one
  universal capability. Native Image preparation includes compiler/native-toolchain
  inputs and relevant reachability/resource configuration. Retain the produced
  executable and required resources/libraries with declared target compatibility;
  final execution need not require the builder's JDK. A guest-language permission
  model is additional defense, not evidence of Attune host isolation.

These are contract coverage cases, not a promise to ship every definition in the
first delivery. Bun and requirements-based uv remain the initial rollout. Deno,
one Gradle-built JVM target, and one Native Image target must inform the phase-1
contract and pass their selected acceptance cases before being advertised as
supported; PyPy remains optional. Avoid building a general dependency/task graph
engine into Attune when a pinned ecosystem helper can own its native semantics.

Authoritative ecosystem references for implementation:
[Deno modules](https://docs.deno.com/runtime/fundamentals/modules/),
[Deno permissions](https://docs.deno.com/runtime/fundamentals/security/),
[Gradle directories](https://docs.gradle.org/current/userguide/directory_layout.html),
[Gradle Wrapper](https://docs.gradle.org/current/userguide/gradle_wrapper.html),
[Gradle daemons](https://docs.gradle.org/current/userguide/gradle_daemon.html), and
[GraalVM Native Image](https://www.graalvm.org/latest/reference-manual/native-image/).

Language-specific switches belong in these definitions/helpers. Generic policy
enforcement must remain effective even if a helper ignores every requested switch.

## Security requirements

### Trust and isolation

The adversaries are a malicious or compromised pack/dependency, a caller trying to
alter admitted execution, and one workload attacking another through a shared
host/cache. Trusted operators control worker images, pool admission, and policy.
Privileged host compromise is outside the isolation claim.

Use separate execution profiles for discovery, preparation, and final workloads.
Profiles fix allowed mounts, credentials, network, resources, and writable roots.
Recipes cannot weaken them, select a privileged profile, or fall back to an
unsandboxed launch after enforcement failure.

The following matrix defines delivery guarantees. **Required** is an eligibility
gate; **outside guarantee** means the profile explicitly trusts the relevant code
and does not claim enforcement. A configured additional requirement always becomes
an eligibility gate, even in a trusted profile. No required capability may be
silently downgraded. The hostile profile is **unavailable until implemented and
validated**; its column is an acceptance target, not current behavior.

| Capability / phase | Initial trusted profile | Trusted profile with Bubblewrap | Hostile-pack profile (future) |
| --- | --- | --- | --- |
| Exact pool admission, immutable bindings, stdin JSON, cleared environments, no setup/probe API token | Required in every phase | Required in every phase | Required in every phase |
| Phase deadlines, bounded output, cancellation accounting | Required; uncertain teardown is reported/quarantined | Required; uncertain teardown is reported/quarantined | Required, with enforced process-tree teardown |
| Probe/setup host filesystem and sibling isolation | Outside guarantee; code/dependencies trusted | Separate sandbox with read-only inputs and private writable staging | Required |
| Final workload mount/process isolation and read-only runtime | Outside guarantee unless separately required | Required; current sandbox prerequisites apply | Required |
| Publication fencing, distinct build generations, ownership records | Required | Required | Required |
| OS prevention of cross-generation/cache modification | Outside guarantee; one admitted shared trust scope | Sandbox writes limited to the current build or invocation | Required for all pack-controlled phases |
| OS CPU/memory/disk/process-count quotas | Required only when explicitly selected and supported; otherwise outside guarantee | Same; current sandbox alone does not supply all quotas | Required in every phase |
| Destination-restricted setup network / invocation egress | Outside guarantee unless separately selected and enforced | Outside guarantee; current sandbox shares networking | Required according to explicit operator network policy |
| Manager master credentials inaccessible to every pack-controlled phase | Environment/parent protections required; full host isolation outside guarantee | Pack processes separated from manager environment, descriptors and host files during sandboxed phases | Required, including setup/discovery and sibling-launcher protection |

Bind the selected profile, its version/digest, and any additional requirements to
capability advertisement, placement, preparation identity, and captured invocation
context. An unsupported required capability makes the pool ineligible. Trusted
profiles admit only operators' trusted definitions/dependencies; neither profile
claims to contain a malicious installer. Initial delivery must clearly display
that trust contract. Private registry credentials are unavailable until the
separate delivery controls below are implemented for the selected profile.

Phase deadlines cover probes, builds, action timeouts, and process startup/stop.
Long-running sensors retain their supported supervised lifetime; bounded health
and shutdown deadlines do not impose an arbitrary total sensor runtime limit.

Discovery and preparation need their own OS-enforced isolation before supporting
untrusted definitions/dependencies. They receive read-only necessary tool/pack
inputs, private scratch, and scoped writable output. They cannot inspect manager
config, custody spools, unrelated runtime roots, sibling processes, or host service
credentials. Close unrelated inherited descriptors. Parent dumpability protection
and cleared environments remain defense in depth, not filesystem/process isolation.

The initial trusted deployment milestone may retain documented setup/verification
limitations, but must not enable automatic admission of arbitrary recipes. Existing
final-process isolation remains opt-in until deployment prerequisites are verified.
Mutually hostile pack support additionally requires isolated setup/discovery,
protected runtime publication/cache storage, and removal or effective separation
of manager-held master credentials from every pack-controlled process.

### Filesystem, commands, and resource limits

Validate canonical owned roots and reject traversal, broad mounts, special-file
inputs, and symlink escapes. Path validation must resist replacement between check
and use; prefer descriptor-relative operations and enforced mounts. Validate build
outputs before exposing them to another process or publishing readiness.

Use trusted executable lookup for launchers; runtime-controlled loader variables
must never affect the host sandbox launcher. Apply deadlines and byte/CPU/memory/
disk/process limits to all phases where required by the profile matrix, including
child hooks. Cancellation targets the owned process tree and awaits completion;
killing the immediate child alone is insufficient. Trusted profiles must surface
unconfirmed cleanup and quarantine outputs; protected-profile tests prove enforced
teardown. Unsupported required enforcement fails closed.

### Network, dependencies, and credentials

Definitions disable implicit fetching of missing dependencies/interpreters during
final invocation. This is a recipe convention in trusted profiles; arbitrary code
can still use their available network. Profiles claiming restricted acquisition
or egress require OS/deployment enforcement outside the recipe, with explicit
permitted destinations and failure on unsupported enforcement. Account for
redirects, DNS changes, proxies, and internal metadata/control-plane endpoints.
Do not claim egress isolation from package-manager flags or current Bubblewrap mode.

Use lockfiles, exact tool/image pins, integrity checks, and defined registry
precedence to reduce supply-chain and dependency-confusion risk. Package build
hooks are arbitrary code. Disabling some hooks is useful defense in depth, not a
replacement for isolation. Bound archive extraction and validate links/paths.

Discovery/setup receive no execution or sensor API token. Where private dependency
registries require credentials, use separately authorized, short-lived preparation
credentials with scoped destinations, private delivery, and no persistence in
published environments/caches. If that cannot be enforced, private-registry support
remains unavailable for the affected profile. Audit identifiers, not secret values.

Clear inherited environments and select explicit config/HOME/cache roots. Prevent
ambient parent-directory, user, system, and `.env` discovery from introducing
undeclared inputs or credentials. Keep existing caller-override allowlists and
protected-variable checks; a generic design must preserve current defenses.

### Admission, authority, and observability

Pin runtime definition and helper content through the workload/provider release
bindings alongside the admitted executable selection. Later authoring changes
cannot alter queued work, retries, or sensor
generations. Alias conflicts do not transfer ownership or deployment permission.
Runtime discovery grants neither API permissions nor scheduling privilege.

Carry execution authority, attempt fences, permission snapshots, input origins,
output secrecy, and encrypted log custody through the shared invocation module.
An absent grant or revoked admission never authorizes a fallback. Setup logs and
errors are bounded and access-controlled; registry tokens, credential-bearing URLs,
parameters, and decrypted values never enter audit or ordinary service logs.

Emit phase, definition/release identity, worker identity, policy identity, attempt,
timing, failure category, and nonsecret environment identity. A policy denial must
be distinguishable from a missing tool or installation failure.

## Implementation sequence and completion gates

1. **Finalize contract and inventory consumers.** Map worker, sensor, API candidate
   tests, bootstrap, CLI/MCP, UI authoring, snapshots, and placement paths. Specify
   discovery/version extraction, input selection, templates, capability records,
   profiles, failure states, workload/provider release bindings, writable build
   workspaces, prepared launch targets, and separate build/execution requirements. Identify
   persisted changes and generated-client effects. Done when every consumer has an
   explicit transition, profile requirements, artifact retention, and trust decision.
2. **Extract shared lifecycle behavior.** Route existing Python/Node/shell/native
   behavior through the module before adding new runtimes. Preserve native direct
   execution, version constraints, stdin/output contracts, environment claims, and
   secret/log handling. Remove duplicated installers only after parity evidence.
3. **Replace fixed discovery.** Load pinned admitted definitions before probing;
   connect verified instances to worker-local capabilities and placement. Done when
   a fixture runtime unknown to Rust is discovered and selected without core edits,
   and a nonadmitted definition cannot start a probe.
4. **Generalize preparation identity and publication.** Implement multiple inputs,
   recipe/tool/policy fingerprints, trusted readiness, lease fencing, lifetime pins,
   distinct physical build generations, output validation, build-descendant teardown,
   quarantine, and owned cleanup. Done when
   input/tool/policy changes invalidate reuse and failed, partitioned, or concurrent
   builders cannot publish a partial or stale generation.
5. **Enforce phase profiles.** Preserve existing trusted mode constraints and add
   discovery/preparation enforcement. Document supported platforms, mounts, artifact
   transport, kernel/tool prerequisites, and egress capability. Complete hostile-pack
   gates before enabling that trust claim; disabling checks is not completion.
6. **Add runtime/toolchain definitions.** Deliver Bun and requirements-based uv
   preparation; then locked uv projects. Use Deno, a Gradle/JVM application, and
   Native Image as broader contract acceptance cases before claiming ecosystem
   support; PyPy remains optional. Exercise actions, sensors, and candidate tests
   where supported. No language branches enter the core module.
7. **Roll out deliberately.** Update docs and applicable AGENTS architecture pointers,
   Compose, release deployment configuration, and canonical sibling chart definitions.
   Inventory old environments and drain/pin users before reclaiming them. Make
   unsupported pools ineligible, fail closed on missing enforcement, and retain the
   ability to disable new definitions without mutating already admitted selections.

## Validation and acceptance

Follow [Running Tests](../testing/running-tests.md) for runners, owned disposable
infrastructure, concurrency, CI, result reuse, and validation handoff. This plan
alone is documentation; implementation requires affected Rust and boundary tests.

- **Generic contract:** an unknown fixture runtime with a deliberately different
  version format, multiple tool probes/input files, and invocation arguments works
  without core changes. Missing inputs, malformed probes, oversized output, alias
  conflicts, and unsupported capabilities reject deterministically.
- **Build/launch contract:** exercise source, JAR/classpath, application directory,
  and native outputs through the same interface. Verify writable workspace isolation,
  retained resources, forged launch metadata rejection, wrong-platform/ABI or missing
  library rejection, entrypoint/source-only cache invalidation, private inline builds,
  and absence of a compiler on a compatible execution host where distribution is
  explicitly supported. Initially reject unsupported cross-pool targets.
- **Parity:** existing actions, native entrypoints, sensors, version placement,
  candidate tests, inline imports, retries, cancellation, and stdout/stdin behavior
  retain their captured contracts. All phases use the same lifecycle implementation.
- **Preparation:** lock/config/helper-only edits invalidate reuse; wrong-image/tool
  instances cannot reuse environments; lease loss, concurrent builders, crashes,
  cleanup, and active readers preserve publication and ownership invariants. A
  partitioned manager with surviving descendants cannot publish its old generation,
  and a replacement never reuses that physical path. Test quarantine and later
  cleanup with positive proof of writer termination or external fencing.
  A build helper exiting while a daemon still writes cannot publish; no descendant
  can mutate a protected generation after publication. Test daemon scope separation
  and per-invocation writable scratch without altering retained executable content.
  Change operator build environment/configuration while keeping pack release,
  recipe, and image fixed: reuse must invalidate or be disabled. Exercise credential
  policy/access revisions without exposing secret values in cache keys or logs.
- **Cross-pack bindings:** a workload in A uses runtime helpers from B's pinned
  release after B is updated or its authoring records are deleted. Verify authenticated
  distribution, correct helper-root resolution, retention, explicit revocation, and
  cleanup across actions, sensors, retries, queued runs, and candidate tests.
- **Adversarial isolation:** use owned dummy credentials and sibling processes to
  test manager/sibling inspection, signaling, writable-runtime poisoning, traversal,
  symlink races, descriptor leakage, launcher-variable injection, and cleanup scope.
  Test setup hooks and probes, not just final workloads. Required unavailable
  isolation is a failing selected case, not passing evidence or silent fallback.
- **Authority/secrets:** no-token setup/probes, exact caller overrides, revoked
  admissions, immutable retries, output policy, secret origins, private registry
  failures, log custody, and audit redaction cross real affected boundaries.
  Reject invocation-input-dependent lifecycle compilation while that feature is
  gated. Later enablement requires tests for attempt-fenced secret delivery,
  diagnostics/intermediate custody, retry/disposal, shared-cache exclusion, and
  unchanged schema-driven classification of final action outputs.
- **Network/resources:** external acquisition during invocation fails under the
  profile that requires enforced restrictions; trusted profiles verify recipe
  settings and explicitly report available network as outside their guarantee.
  Test the profile matrix, placement rejection, setup egress, process-tree
  cancellation, output floods, disk exhaustion, and timeout behavior on owned
  infrastructure. Distinguish required hard limits from budgets a profile cannot enforce.
- **Runtime definitions:** JS/TS and dependency imports, requirements and locked
  Python projects, optional PyPy compatibility/version identity, read-only packs,
  private writable scratch, and separate action/sensor pools are covered. Add
  Deno frozen/cached dependency graphs and permission denials; Gradle wrapper
  integrity, daemon teardown, JAR/resource launches and explicit task invocation;
  Native Image compatibility/resources and separate build/run tool requirements.
  Use offline pinned fixtures or owned dependency servers for deterministic tests.
- **Release checks:** applicable formatting, check/Clippy, targeted integration/E2E,
  zero new warnings, and required CI. Contract/version changes require web API
  generation/check and matching versions; schema changes require SQLx preparation.

No speedup or security claim is accepted from omitted tests, hidden retries,
unavailable prerequisites, or dependency-manager marketing benchmarks. Record the
tested revision, trust profile, deployment prerequisites, checks, and remaining gaps.

## Decisions to resolve before implementation

- Exact definition schema, bounded version extraction, and identity matching rules,
  including non-semver tools and separate interpreter/implementation versions.
- Initial operator admission UX and how definition updates/revocations affect new
  probes, builds, queued invocations, retries, and already running workloads.
- Isolation implementation for probes/builds; whether the hostile-pack milestone
  needs a separate builder pool and attempt-bound control APIs before activation.
- Private registry credential delivery and egress enforcement availability.
- Versioning of effective operator build configuration and credential access scope;
  unclassified mutable inputs disable reuse. Invocation-input-dependent compilation
  stays unavailable pending a separate attempt-bound authority/secrecy contract.
- Worker/image identity, shared-volume topology, trust-scope cache separation, and
  trustworthy publication metadata, including disposal of older writable caches.
- Locked project/workspace semantics, optional inputs, and policy for unlocked
  development packs versus immutable production releases.
- Exact prepared-target/output schema, entrypoint/source mapping, writable-copy
  strategy, per-invocation scratch, and same-worker-only delivery constraints.
- Target platform/ABI/shared-library verification and the later authenticated
  cross-pool distribution/retention contract; automatic cross-compilation is not
  assumed. Define tool-specific requirements in recipes and generic comparison data.
- Operational budgets, environment pinning/retention, and rollout across Compose,
  release artifacts, and the canonical chart repository.

## Review record

An independent read-only GPT-6 ASTRA review examined the original draft and linked
implementation/security contracts for completeness, coherence, and security. It
found the overall direction sound and identified three corrections. All three are
incorporated into this revision:

1. **High: cross-pack helper artifact binding was underspecified.** Added separate
   workload/provider release pins, helper-root namespaces, authenticated artifact
   distribution, retention obligations, and update/delete/revocation tests.
2. **High: trusted rollout exceptions conflicted with unconditional enforcement
   language.** Added the phase-by-profile requirements matrix, profile-bound
   placement/acceptance, explicit outside-guarantee behavior, and matching network,
   filesystem, resource, and teardown requirements.
3. **Medium: lease expiry did not prevent surviving remote filesystem writers.**
   Added permanent generation-specific directories, trusted logical-to-physical
   publication, quarantine of uncertain generations, isolation against cross-path
   writes in protected profiles, and partitioned-manager tests. Reclamation needs
   teardown evidence or verified external fencing.

The reviewer considered the remaining bootstrap, version extraction, admission/
revocation, private-registry, and platform decisions adequately gated, rather than
resolved. It recommended an explicit primary version for constraint selection;
that requirement is now included. Residual risks include trusted unsandboxed hooks,
unlocked dependencies, mutable tools, and builder access to registry credentials.

### Second review: broader toolchains

A fresh independent read-only GPT-6 ASTRA review examined draft SHA-256
`88fcb8dcc7a06c268f805b716db89ff92825d0f4e587f5c0fb78620ecd70f746`, including
prepared artifacts, writable build workspaces, separate build/run requirements,
daemon ownership, Deno, Gradle, and GraalVM coverage. It confirmed that the three
earlier findings are addressed in the proposed contract and found the broader
ecosystem model coherent. It identified two medium-priority corrections, both
incorporated after that review:

1. **Attempt-private secret-dependent compilation lacked a lifecycle contract.**
   Initial preparation now excludes rendered invocation inputs and tokens. The
   dependent-compilation feature stays unavailable until attempt-bound input
   origins, private diagnostics/intermediates, retries/disposal, and cache exclusion
   are specified and validated. Final output classification retains its existing
   schema/channel contract; private build storage alone is not a security proof.
2. **Reuse identity omitted effective operator build configuration.** Added an
   immutable configuration revision, classification of all exposed settings,
   invalidation/no-reuse defaults, and separately revisioned credential/source trust
   partitions without public secret hashes. Added corresponding acceptance cases.

The reviewer considered the remaining explicit contract/platform decisions
adequately gated. The author incorporated the two corrections afterward; the next
review examined that corrected revision.

### Third review: final corrections

A fresh independent read-only GPT-6 ASTRA review examined exact draft SHA-256
`850ac5efdfc9f36f61f1a34c68b891f2a8257ad2d4768078d62822928acae889`.
It found no concrete actionable defects or implementation-blocking proposal gaps.
The reviewer confirmed both second-review findings are addressed:

- Invocation-input-dependent compilation is explicitly gated, initial preparation
  excludes invocation inputs/tokens, and later enablement requires attempt-bound
  authority, private intermediates/disposal, and shared-cache exclusion. Final
  action output classification remains schema-driven.
- Effective build configuration and credential access scope are revisioned in reuse
  identity, unclassified/mutable inputs disable reuse, and acceptance tests cover
  invalidation without exposing secret values through public hashes.

The reviewer found the source/build/output model, release bindings, publication
fencing, retention, placement, and profile guarantees coherent. Exact schemas,
admission/revocation behavior, configuration revision mechanisms, private-registry
delivery, isolation prerequisites, and compatibility checks remain pre-implementation
decisions. This supports proceeding to contract finalization, not claiming a secure
implementation. Only this review record changed after the reviewed draft; no design
requirements were changed in response to that review. No runtime or infrastructure
tests were run as part of the proposal review.

## Initial implementation scope

The first implementation adds a closed `execution_config.lifecycle` recipe
contract and a shared `attune_common::runtime_lifecycle` module used by actions,
sensors, and explicit runtime candidate-test runners. Exact recipe digests are
operator-admitted at manager startup, include caller-environment policy, and are
advertised under owner-qualified runtime refs for action/sensor placement. Discovery
checks all required tools; invocation rechecks the selected primary version.
Captured parent configuration is passed through even when no version override is
selected, so authoring edits cannot replace an admitted parent runtime implicitly.

Each preparation owns a fresh private physical generation and a validated launch
target. Workspace copies preserve source-relative structure and keep inputs
immutable. Setup/probe commands have bounded time/output and process-group cleanup;
ordinary child environments and final-process authority/logging remain integrated.
Normal joined completion removes the generation only after process-group exit is
confirmed; unreaped or unconfirmed descendants retain quarantine. Failures/dropped preparation leave
quarantine rather than authorize reuse. Unix process groups supervise trusted
descendants; deliberate group escape is not claimed to be contained.

Bun, requirements-based uv, locked uv projects, Deno, and optional PyPy are authored
as recipe data. Gradle/JVM and Native Image are illustrative build/launch recipes.
The optional Compose toolchain pool provisions Python, Node, Bun, and uv, with no
recipes admitted by default. See [operator instructions](../deployment/runtime-toolchains.md).

This does not complete every later stage in the design. Existing interpreter
definitions and their setup paths remain supported during the staged extraction.
Prepared-target reuse/publication coordination, external provider helper artifacts,
cross-host distribution, hostile profiles, restricted egress, private registry
credentials, and invocation-input-dependent preparation remain unavailable in the
new recipe path. In particular, not reusing generations is the conservative initial
implementation of configuration invalidation, not evidence that shared cache or
lease fencing has been completed. New recipe consumers do not use legacy installer
adapters or add a second snapshot format. Independent implementation reviews and subsequent validation are recorded below.
Required CI still gates merge readiness.

### Local validation checkpoint — 2026-11-17

Status: **work in progress**, not ready to merge. This checkpoint is the initial
trusted-profile delivery above, not completion of all later implementation gates.
Validation used uncommitted changes based on
`818192b256e0cf683b356f0b7d7c3156b28cdacc`.

- `cargo check --all-targets --workspace --offline`: passed against the final Rust
  source with zero warnings. Formatting and `git diff --check` passed.
- Focused tests: 13 shared lifecycle cases, 23 runtime repository cases, three
  sensor admission cases, three worker recipe cases, and six scheduler runtime
  selection cases passed (48 distinct cases). Direct test runs used unique run IDs
  and four test threads. Database cases used an owned disposable PostgreSQL 16
  cluster; no client sessions remained before its awaited shutdown and removal.
- Repository/admission results were reused after changes confined to process
  teardown and private probe-home retention. Worker/scheduler results were reused
  after the final probe-home retention correction; the final 13-case lifecycle run
  covers that correction, and the final workspace check covers its callers.
- `npm run generate:api` and `npm run check:api`: passed. Workspace, exported
  OpenAPI, and generated client versions match `0.11.1`; no generated client diff
  was needed. Compose's optional toolchain profile passes configuration validation.
- The optional image built on Linux AMD64. Command-level offline smoke checks read
  the bundled Bun and uv recipes and passed with Bun 1.4.4, uv 0.9.10, and Python
  3.12.16, including frozen local dependencies/locks, local imports, stdin JSON,
  and unchanged input trees. These ran as an unprivileged user with networking
  disabled and a read-only container root. They are separate from Attune's runtime
  policy guarantees. The owned container/image tag were removed. Bun/uv manifests
  also advertise ARM64; execution on ARM64 was not tested.

Gaps: required CI, full-stack worker/sensor/candidate service coverage, independent
implementation/security review, and actual PyPy, Deno, Gradle/JVM, and GraalVM
compatibility runs remain outstanding. The broader deferred implementation gates
listed above remain unavailable. No existing service was restarted or deployed,
and no change was committed, pushed, merged, or published.

### PR preparation — 2026-11-18

The runtime changes were transferred onto current `main`
(`e93077ac9102a025d732fb5d54565e684904a94c`) without carrying the original
Dependabot branch's commits. Git merged the changes without textual conflicts,
including the upstream action command-launch work. A fresh all-target workspace
check is running on this base. The preceding 48-case results and image smoke
checks describe the earlier checkpoint; they do not certify this new base or
replace required CI. The PR remains a draft with the integration, review, and
compatibility gaps listed above.


### Readiness remediation — 2026-11-18

The initial delivery received independent read-only Standards/security and Spec
reviews, followed by rechecks. Standards findings were version-alias collisions
and preparation timeouts bypassing the action budget. Spec findings were mixed
legacy/lifecycle version selection, alternate versions depending on the default
executable, and inline sources missing the action boundary/private input root.
These are corrected with owner-qualified version access, mode checks before
verification and selection, independent alternate discovery, one phase budget,
and explicit inline source mapping. Rechecks also led to normal timeout/cancelled
terminal results, omission of nonexistent sandbox input roots, and managed-only
pack-file lookup. No additional concrete authority or disclosure defect was found
within the reviewed trusted profile. The reviews were static, not penetration tests.

Candidate routing now carries exact recipe requirements in the existing v1 test
request and selects only workers advertising those startup operator admissions.
The worker independently verifies the archived recipe and tools. This closes the
previous possibility of dispatch to an unadmitted pool without introducing a
language-specific scheduler or changing execution authority.

Current command-level fixtures passed on Linux AMD64 with Bun 1.4.4, Python
3.12.16/uv 0.9.10 (requirements and locked projects), PyPy implementation 7.3.20
with Python 3.11.13, Deno 2.5.6, Gradle 8.14.3/Java 21.0.9, and GraalVM Native
Image 25.0.1. They cover local imports, frozen locks, JAR/native resources, stdin
JSON, immutable inputs, and Deno permission denial without network access.
GraalVM's JVM temporary helper path is explicitly owned scratch. These checks are
repeatable with `scripts/test-runtime-toolchains.py`; they do not certify arbitrary
workloads, other architectures, or cross-host ABI compatibility.

The CI full-stack runner now has a fresh-stack-only recipe fixture mode. It
stages an owned copy of core, admits two fixed digests in the selected test pools,
and exercises released and inline actions, preparation timeout classification,
sensor events/rules, and admitted/rejected candidate tests via the public API.
It does not alter the repository's shipped core components. Rust tests verify the
fixture digests and cover scoped versions, absent default executables, mode
rejection, preparation budgets/cancellation, and actual sensor bootstrap/cleanup.
The Docker Rust test image includes runtime-read recipe fixtures and examples.

Status remains work in progress until the updated tests and required CI pass on
the final PR head. ARM64 execution and all broader deferred architecture milestones
remain outside this initial delivery's validated guarantees. The PR handoff holds
current command results and CI state; older checkpoints above are historical.

### Install-time preparation follow-up

The opt-in slice in [runtime toolchains](../deployment/runtime-toolchains.md#install-time-preparation)
uses the pinned candidate dispatch and keeps the pack activation transaction
closed while the worker prepares its generations. Activation rechecks the same
projected executable targets and eligible storage scope before publishing ready
references. File-backed actions and managed sensors open the resulting content
read-only. Retained-file and tool verification fails closed if the generation
cannot be used on the selected manager. Inline and candidate test invocations
retain private, per-invocation preparation.
