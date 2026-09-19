# Pack release artifacts and external core specification

**Status:** Proposed specification

**Last updated:** 2026-09-16

**Target:** Attune pack release format 1 and pack index format 1

**Pre-production version policy:** These are the initial canonical formats, not a
second generation of a released contract. The embedded manifest uses
`attune.pack.release/v1`, the archive media type uses `pack.v1`, and the catalog
uses `format_version: "1.0"`. Replace superseded development formats rather than
shipping dual parsers or compatibility adapters. Explicit, safe data reset or
conversion may be required for development deployments; this does not establish
a legacy runtime format. Pack release versions (for example `1.1.0`) and the
Attune application version are independent of these format identifiers.

## Summary

This specification defines how a pack author publishes actions, sensors, Java
archives, and native executables that an external CI pipeline builds outside the
pack's Git repository.

A published pack release is one immutable, self-describing archive. The archive
contains the pack definition and every file that Attune may execute. It may
contain several native variants for a logical executable. Attune validates the
archive at ingestion, stores its exact bytes, and verifies it at materialization.
Action assignment and sensor lease acquisition select compatible variants.

This specification also separates built-in platform metadata from the indexed
`core` pack. Attune keeps basic runtimes, administrative permission sets,
intrinsic handlers, and system event contracts available before any pack is
installed. All current core actions and the timer sensor move into an
index-installable `core` release without changing their public `core.*` refs.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative.

## Problem

The current pack paths assume that Git source, uploaded files, and executable
release content are the same thing. That assumption fails for native programs
and Java archives built by CI. Pack authors often do not commit generated
binaries, and `attune pack upload` honors ignore files that commonly exclude
those binaries.

Archive transport also loses executable intent on some ingestion paths. The
initial TAR and ZIP extractor creates files without restoring executable bits.
The release publisher can only record the permissions that survive ingestion.
The worker cannot recover intent that the installer discarded.

Native releases have another dimension. A Linux `amd64` binary cannot run on a
Linux `arm64` worker. The current action and sensor model names one literal
entry-point path and has no platform selection contract.

Java content has different requirements. A JAR is a normal `0644` file launched
by the Java runtime. An executable JAR needs `java -jar`. Compiled classes need a
classpath and a main class. The current interpreter model always appends one
action file and cannot express both launch forms cleanly.

The current `core` pack also combines two lifecycles:

- Platform metadata needed to authorize and start Attune.
- Versioned executable content that should install through the normal pack path.

This causes a bootstrap cycle. The installer needs `core.admin` to upload the
pack that currently defines `core.admin`. The existing bootstrap path breaks the
cycle by loading core content directly into PostgreSQL before using the API.

## Goals

This feature MUST provide these outcomes:

- A CI pipeline can assemble a pack release from Git-tracked definitions and
  generated files that are absent from Git.
- The same release bytes have the same digest in CI, the index, the API, object
  storage, and worker caches.
- Executable intent does not depend on source filesystem permissions or archive
  extraction defaults.
- One installed release can run on a mixed Linux `amd64` and `arm64` fleet.
- Native selection is deterministic, validated, and recorded before execution.
- Java source, executable JAR, and compiled-class launches have distinct typed
  contracts.
- Pack installation, update, retry, and rollback do not rebuild release content.
- A clean cluster can install required packs from a configured index without a
  direct pack database loader.
- All current core actions and the timer sensor can come from an indexed release.
- Basic runtime and authorization metadata exists before indexed packs.
- Existing `core.*` component refs remain stable.
- Air-gapped operators can install the same archive through a local upload or an
  internal index.

## Non-goals

The first release does not provide these capabilities:

- Building source code inside the Attune cluster.
- Downloading executable files when an action starts.
- Dynamic native dependency installation.
- Bundled JREs or other language runtimes.
- Windows or macOS native execution.
- CPU feature selection such as AVX2.
- GPU or accelerator ABI selection.
- Delta archives or shared binary layers.
- Vendored Python, npm, Maven, or Gradle repositories.
- Mandatory Sigstore, GPG, or enterprise PKI verification.
- General scheduler plugins supplied by packs.

## Terms

**Pack source** is the authoring directory. It contains `pack.yaml`, component
definitions, scripts, tests, and optional source files. It does not need to
contain generated release artifacts.

**Release input map** maps logical artifact names and target selectors to files
produced by CI.

**Canonical pack release archive** is the exact `.attune-pack.tar.gz` file that
the publisher signs or checksums, the index references, and Attune stores.

**Release manifest** is `.attune/release-manifest.json` inside the archive. It
lists every payload file and every logical artifact variant. A payload file is
any regular file except the manifest itself. The manifest never hashes itself.

**Logical artifact** is a stable name used by component metadata, such as
`timer_sensor` or `reconcile_jar`.

**Artifact variant** is one concrete file for a logical artifact. A native
variant includes a target selector. A portable JAR has one target-independent
variant.

**Platform catalog** is built-in metadata reconciled by the Attune application.
It is not a pack release.

**Indexed core release** is the normal immutable pack release that owns core
actions, timer triggers, and the timer sensor.

## Pack author workflow

### Build release artifacts outside Git

The pack repository declares logical artifacts in component YAML. CI builds the
files and passes them to the release builder.

For a native timer sensor:

```yaml
ref: core.timer_sensor
label: "Timer Sensor"
description: "Fires timer events"
enabled: true
runner_type: native
launch:
  type: native
  artifact: timer_sensor
trigger_types:
  - core.intervaltimer
  - core.crontimer
  - core.datetimetimer
  - core.rruletimer
```

CI assembles the release:

```sh
attune pack release build ./core \
  --artifact 'timer_sensor[linux/amd64/static]=dist/linux-amd64/attune-core-timer-sensor' \
  --artifact 'timer_sensor[linux/arm64/static]=dist/linux-arm64/attune-core-timer-sensor' \
  --output dist/core-1.1.0.attune-pack.tar.gz

attune pack release verify dist/core-1.1.0.attune-pack.tar.gz
```

The release builder MUST include supplied artifacts even when `.gitignore` or
another ignore file excludes their source paths. Ignore processing applies only
while collecting the pack source directory.

### Build an executable JAR action

```yaml
ref: acme.reconcile
label: "Reconcile"
description: "Reconciles Acme resources"
enabled: true
runner_type: java
runtime_version: ">=21"
launch:
  type: java_jar
  artifact: reconcile_jar
parameter_delivery: stdin
parameter_format: json
output_format: json
```

```sh
attune pack release build ./acme \
  --artifact reconcile_jar=build/libs/reconcile.jar \
  --output dist/acme-2.4.0.attune-pack.tar.gz
```

The worker executes this action as:

```text
java -jar <absolute-path-to-reconcile.jar>
```

The JAR uses mode `0644`. The Java runtime executable carries execute
permission.

### Publish the release

The publisher uploads the canonical archive to ordinary immutable HTTP object
storage and adds the release descriptor to an index:

```sh
attune pack index release add \
  --index index.json \
  --archive dist/acme-2.4.0.attune-pack.tar.gz \
  --url https://packs.example.com/acme/2.4.0/acme-2.4.0.attune-pack.tar.gz
```

Git MAY appear as provenance. Git MUST NOT be the fallback installation source
for a canonical release because the repository may not contain the release
artifacts.

## Canonical archive format

### Media type and file name

The media type is:

```text
application/vnd.attune.pack.v1+tar+gzip
```

The recommended suffix is:

```text
.attune-pack.tar.gz
```

### Archive layout

The archive MUST contain one top-level directory named after the pack ref:

```text
core/
|-- pack.yaml
|-- actions/
|-- sensors/
|-- triggers/
|-- tests/
|-- .attune/
    |-- release-manifest.json
    |-- artifacts/
        |-- timer_sensor/
            |-- linux-amd64-static
            |-- linux-arm64-static
```

The archive MUST satisfy these rules:

- It uses gzip-compressed POSIX ustar with regular-file entries only.
- Every path is valid UTF-8 in NFC form using Unicode 15.1 normalization.
- Every path uses `/` separators.
- Entries appear in bytewise path order.
- UID, GID, user name, group name, and modification time are zero or empty.
- Gzip modification time is zero.
- Directory entries are omitted. Extraction creates parent directories.
- Regular non-executable files use mode `0644`.
- Declared executable files use mode `0755`.
- Only regular files are allowed as archive entries.
- Absolute paths, `.` components, `..` components, duplicate paths, and
  case-folding collisions are rejected.
- Symbolic links, hard links, sparse files, devices, FIFOs, sockets, and
  extended attributes are rejected.
- `pack.yaml` and `.attune/release-manifest.json` are required.

The archive SHA-256 is the release digest. Attune MUST store conforming archive
bytes without repacking them.

Manifest paths are relative to the pack root, without the top-level pack ref.
Archive paths prepend `<pack-ref>/`. Empty components, control characters,
backslashes, `:`, `;`, `*`, `?`, `[` and `]` are forbidden. Paths are at most 255
UTF-8 bytes including the archive root and must fit ustar's name and prefix
fields. The writer uses an empty prefix when the path fits the 100-byte name
field. Otherwise it splits at the rightmost slash that fits the 155-byte prefix
and 100-byte name fields. Collisions use NFC after Unicode 15.1 full case folding.
File-versus-directory prefix collisions are also rejected.

### Reproducible encoding

The builder serializes the manifest with RFC 8785 JSON canonicalization, without
a trailing newline. Duplicate JSON keys are invalid. It sorts `files` by path,
`artifacts` and `variants` by ID, and set-valued arrays by their canonical UTF-8
bytes. Ordered launch arguments and classpaths preserve their declared order.
Payload bytes, including YAML and line endings, are never normalized.

The ustar writer uses magic `ustar\0`, version `00`, typeflag `0`, empty link,
user, and group names, and zero device fields. Numeric fields use zero-padded
octal with a NUL terminator. The checksum uses six octal digits, NUL, and space.
Unused bytes and file padding are zero. Exactly two zero blocks end the TAR,
with no record padding. PAX records and GNU extensions are not supported.

The gzip stream has one member, no optional fields, MTIME zero, XFL `2`, and OS
`255`. The supported builder uses the pure-Rust `miniz_oxide` encoder at level 9,
with its version pinned in the builder's Cargo lockfile. Format-1 publication
MUST NOT ship until Linux and macOS builds pass shared golden archive vectors.
An encoder upgrade must preserve those vectors or introduce a new explicitly
versioned builder encoding profile. Reproducibility means identical input bytes
and the same encoding profile, not arbitrary third-party compressor output.

The validator checks container structure and metadata, but does not recompress
an archive to compare DEFLATE decisions. Exact-byte identity is established by
the publisher digest. Non-reproducible producers cannot claim builder conformance.

Directories, Git sources, ZIP files, and ordinary TAR archives MAY be supported
as explicit development/source inputs, not as an older canonical format that
must remain compatible. Attune canonicalizes supported inputs into a release.
They are not canonical publisher artifacts.
Source extraction may normalize `./` paths. Canonical-v1 validation must reject
them rather than silently rewrite signed or digest-pinned content.

## Release manifest

### Schema

The manifest uses strict JSON. Unknown fields are rejected for schema version 1.
Superseded development storage manifests are not a supported alternate wire
format. Reset or explicitly convert development data before using this schema. The following is an abbreviated field example,
not a complete core release inventory.

```json
{
  "schema": "attune.pack.release/v1",
  "pack": {
    "ref": "core",
    "version": "1.1.0"
  },
  "requires": {
    "attune": ">=0.9.0,<1.0.0",
    "platform_catalog": ">=1",
    "runtimes": ["core.native", "core.shell", "core.python"]
  },
  "dependencies": {},
  "source": {
    "repository": "https://github.com/attune-system/attune-core-pack",
    "revision": "0123456789abcdef0123456789abcdef01234567"
  },
  "files": [
    {
      "path": "pack.yaml",
      "size": 1024,
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "mode": "0644"
    },
    {
      "path": ".attune/artifacts/timer_sensor/linux-amd64-static",
      "size": 4812288,
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "mode": "0755"
    }
  ],
  "artifacts": [
    {
      "id": "timer_sensor",
      "kind": "native",
      "component_refs": ["core.timer_sensor"],
      "variants": [
        {
          "id": "linux-amd64-static",
          "path": ".attune/artifacts/timer_sensor/linux-amd64-static",
          "target": {
            "os": "linux",
            "arch": "amd64",
            "libc": "static"
          }
        }
      ]
    }
  ],
  "evidence": {
    "sboms": [],
    "provenance": []
  }
}
```

### Manifest invariants

The validator MUST enforce these invariants:

- `pack.ref` and `pack.version` match `pack.yaml` exactly.
- Every payload file appears exactly once in `files`.
- The manifest itself MUST NOT appear in `files`. No directory is listed.
- Every listed file exists and matches its size, SHA-256, and mode.
- No unlisted payload file exists.
- Artifact IDs match `[a-z][a-z0-9._-]{0,127}` and are unique.
- Every artifact variant path names a file in `files`.
- `component_refs` is generated from parsed component launches. It must equal
  the sorted set of actual consumers, all within the manifest pack namespace.
- A native variant file uses mode `0755`.
- A JAR artifact file uses mode `0644`.
- Two variants of one artifact cannot have the same target selector.
- Source revision and evidence metadata do not affect platform selection.

The database stores the validated manifest. It MUST NOT infer a replacement
manifest after extraction.

### Artifact kinds

Artifacts form a tagged union with exactly two kinds in MVP:

- `native` has one or more variants. Each variant has `id`, `path`, and a required
  `target` with exactly `os`, `arch`, and `libc` fields.
- `jar` has exactly one variant, with `id: portable` and `path`. It has no `target`.

Both kinds require `id`, `kind`, `component_refs`, and `variants`. Variant IDs use
the artifact-ID grammar. The validator rejects unknown fields, empty consumer
sets, unresolved consumers, unused artifacts, and launch-kind mismatches.
Several components may share one artifact. Artifacts are regular files, never
directories. Class directories are covered by ordinary manifest file entries.
JAR validation checks ZIP structure without extracting it and requires a
`Main-Class` manifest attribute when the consumer uses `java_jar`.

### Executable intent

The builder marks native variants and legacy native entry points executable.
Interpreter inputs, Java files, definitions, and evidence are non-executable.
Authors declare helper programs with repeatable `--executable <pack-relative-path>`
arguments to `release build`. The builder rejects missing paths and never infers
execute permission from the source filesystem. Legacy source import preserves
execute bits as explicit input to canonicalization, not as a canonical-v1 rule.

The manifest is authoritative for executable intent. Archive modes must match
it. The extractor verifies content in private staging and publishes these modes:

- `0444` for normal files.
- `0555` for declared executable files and directories.

Setuid, setgid, sticky, and group-writable modes are never retained.
Manifest modes describe archive intent, not the stripped write bits in the
materialized tree. The cache protection requirements below apply in addition
to these filesystem modes.

## Component launch model

### Shared schema

Actions and sensors share process launch variants. Only actions may use
`Intrinsic`:

```rust
enum LaunchSpec {
    File {
        path: PackPath,
    },
    Native {
        artifact: ArtifactId,
    },
    JavaJar {
        artifact: ArtifactId,
        jvm_args: Vec<String>,
    },
    JavaClass {
        main_class: JavaClassName,
        classpath: Vec<ClasspathEntry>,
        jvm_args: Vec<String>,
    },
    Intrinsic {
        handler: IntrinsicHandlerRef,
    },
}

enum ClasspathEntry {
    Artifact { artifact: ArtifactId },
    Path { path: PackPath },
}
```

`launch` and legacy `entry_point` are mutually exclusive. Existing script and
source-file actions continue to use `entry_point` during migration.

One pure launch planner serves action workers and sensor hosts:

```rust
fn plan_launch(
    launch: &ProcessLaunchSpec,
    release: &VerifiedRelease,
    runtime: &RuntimeDefinition,
    platform: &PlatformTarget,
) -> Result<CommandSpec, LaunchError>;
```

```rust
struct CommandSpec {
    program: PathBuf,
    arguments: Vec<OsString>,
    working_directory: PathBuf,
}
```

The planner owns path confinement, artifact resolution, native selection, JAR
arguments, and Java classpath construction. Process runners own stdin,
environment variables, logs, timeouts, cancellation, and exit handling.
`ProcessLaunchSpec` is the non-intrinsic subset of `LaunchSpec`. The executor
handles intrinsic actions without a runtime or `CommandSpec`. `file` requires
an interpreter-backed runtime and a manifest-listed file. `native` requires a
native artifact and the `native` runtime. Both Java variants require `java`.
`jvm_args` defaults to an empty array. Sensors reject intrinsic launches.

### Native launch

The first release supports these target values:

| Field | Values |
|---|---|
| `os` | `linux` |
| `arch` | `amd64`, `arm64` |
| `libc` | `static` |

Workers and sensor hosts advertise a detected target:

```json
{
  "platform": {
    "os": "linux",
    "arch": "arm64",
    "libc": "gnu"
  }
}
```

Selection follows these rules:

1. `os` and `arch` match exactly.
2. `libc: static` matches any libc on the same OS and architecture.
3. More than one matching variant is a release validation error.
4. No matching variant makes the component ineligible for that host.
5. Action assignment selects and persists the artifact ID, variant ID, path,
   and file digest before dispatch. Without a compatible worker, an execution
   remains scheduled with reason `no_compatible_target` until its scheduling
   deadline expires or capacity appears.
6. Sensor workload acquisition uses the same eligibility predicate inside the
   existing fenced lease transaction. It records the variant on that lease
   generation. Renewal must retain eligibility and the recorded selection.
7. The host streams a recheck of the selected file digest before spawning.
8. Action retries retain their selected variant and require a compatible host.
   A new sensor lease generation may select another variant of the same pinned
   release. The prior generation must lose authority to emit events first.

Host target detection normalizes `x86_64` to `amd64` and `aarch64` to `arm64`.
Manifests accept canonical names only. A host reports `gnu`, `musl`, or `unknown`
libc; all accept static variants on the matching OS and architecture.

Archive validity does not depend on live capacity. Optional packs may activate
without eligible hosts and report unavailable capabilities. Required packs need
candidate smoke-test evidence and an eligible host for each configured required
capability at activation. Missing capacity leaves their install waiting, not an
invalid archive. Later capacity loss degrades capability health, not API health.

The installer does not execute a foreign binary to validate it. It MAY inspect
the binary format and machine type as an additional consistency check.

### Java source launch

The existing Java source behavior remains available through `entry_point` and
`core.java`. The worker executes:

```text
java <runtime-args> <absolute-source-path>
```

### Java JAR launch

`java_jar` requires one portable file artifact. The worker executes:

```text
java <runtime-args> <declared-jvm-args> -jar <absolute-jar-path>
```

The launch definition MUST NOT accept action parameters as JVM arguments.
Parameters continue to use stdin or file delivery.

### Java class launch

```yaml
ref: acme.migrate
runner_type: java
launch:
  type: java_class
  main_class: com.acme.Migrate
  classpath:
    - path: classes
    - artifact: migrate_dependencies
```

The worker executes:

```text
java <runtime-args> <declared-jvm-args> -cp <resolved-classpath> com.acme.Migrate
```

Classpath entries MUST remain inside the protected release tree. `artifact`
entries reference `jar` artifacts. `path` entries name a nonempty directory
whose complete regular-file subtree appears in the manifest. Directory artifacts
are not supported. Classpaths are nonempty, ordered, and reject duplicates,
empty entries, wildcards, and host classpath separators in path names.
The launch planner uses the host platform's classpath separator.

`main_class` is a dot-separated sequence of JVM binary-name identifiers without
path separators or leading `-`. Launch construction uses argv, never a shell.
JVM arguments cannot override `-jar`, main class, classpath, module path, or use
argument files. Environment construction removes `CLASSPATH`, `JDK_JAVA_OPTIONS`,
`JAVA_TOOL_OPTIONS`, and `_JAVA_OPTIONS`. External manifest `Class-Path` entries
are rejected. These restrictions make the launch deterministic, not a sandbox
against a JAR that deliberately opens host files or network connections.

### Intrinsic launch

An intrinsic action has release-managed metadata but platform-managed behavior.
Only handlers present in the platform catalog are valid. A pack cannot define a
handler implementation. The scheduler dispatches by the typed handler in the
executable snapshot, not by comparing an action ref with a special-case value.
The catalog defines the handler's parameter contract and allowed component refs.
The current platform catalog defines no intrinsic action handlers. Workflow
inquiries use action-owned API creation plus `wait_for.inquiry`, not an intrinsic
action. A future handler ref would not grant additional permissions.

## Release identity and storage

Three digests have distinct meanings:

- The release digest is SHA-256 over exact compressed archive bytes.
- File digests are SHA-256 values in the release manifest.
- The optional source revision records provenance and is not release identity.

One pack ref and semantic version MUST identify one release digest. Publishing
different bytes under the same ref and version fails. `force` does not bypass
this rule.

Executions, enforcements, queue items, and sensor workloads continue to pin a
pack release. Executable snapshots add the selected artifact variant and file
digest. Existing pinned releases remain valid after an update or rollback.

Each pack-managed action, sensor, runtime, and runtime version records the
release that last defined it. Activation stores immutable action and sensor
snapshots for the incoming release. A workflow snapshot pins each referenced
action to that action's recorded release, so one workflow can use components
retained from different releases. Release retention treats these nested pins as
live references.

The worker cache key remains the release digest. A universal archive is the MVP
format, so every worker may download variants that it cannot execute. Separate
target layers are deferred until measured release sizes justify the extra
identity, storage, garbage-collection, and recovery rules.

### Cache protection and verification

Pack publication is a trusted-code operation. This feature does not sandbox
arbitrary code, protect the host from an installed pack, or defend against a
compromised administrator. It does require release bytes to remain protected
from writes by ordinary action and sensor processes.

A production format-1 host MUST expose releases through either a read-only mount
with no writable alias reachable by children, or a cache owned by a separate
writer identity. In the latter arrangement children cannot change cache modes,
write any ancestor directory, or obtain the writer's credentials. The writer
accepts only authenticated release IDs and expected digests, never arbitrary
paths or replacement content. Child processes must not retain privileges that
can bypass that protection. Startup checks the chosen arrangement and reports
`cache_protection_unavailable` instead of advertising format-1 capability when
it is absent. Injected agents need a pre-provisioned protected cache or separate
writer; running everything under one writable-cache UID is not conforming.

Permissions alone and digest-valued marker files are not proof of immutability.
Each cache fill verifies the archive and every payload file before atomic rename.
The readiness marker records digest, validator version, and cache generation.
An unknown generation or changed validator requires revalidation. Workers may
reuse that verification only while the protection remains in force. The selected
native file is still streamed through SHA-256 before each launch. All other
release files are covered by the protected verified tree, including Java classes.

Shared-volume transport MUST NOT read or hash the whole archive on every action
start. Archive verification occurs at cache admission or revalidation. All
hashing uses bounded buffers; compression, extraction, and filesystem scans run
outside Tokio executor threads. Superseded development releases must be reset or
explicitly re-imported as canonical v1 before use; do not add a legacy validator
for runtime compatibility. Never relabel changed bytes with an old digest.

### Resource budgets

The initial format-1 defaults are:

| Limit | Default |
|---|---|
| Compressed archive | 512 MiB |
| Extracted payload plus manifest | 1 GiB |
| One regular file | 256 MiB |
| Regular-file entries including manifest | 50,000 |
| Manifest | 16 MiB |
| Logical artifacts | 1,024 |
| Variants per artifact | 16 |
| Concurrent cache fills per host | 2 |
| Local release cache budget including staging | 4 GiB |

These are configurable limits, not promises that every host has that capacity.
One versioned policy feeds upload, registry fetch, validation, storage, and
worker extraction. Hosts advertise their effective limits and free cache budget.
Admission excludes hosts whose limits cannot accept the release. Policy changes
do not silently change limits on existing hosts. Required-pack activation checks
that at least one eligible host accepts the complete archive for each required
capability. The installer records compressed size, extracted size, largest file,
and entry count from verified content, not just index claims.

Readers count actual bytes while streaming and enforce limits before each write.
Declared sizes allow early rejection but cannot replace actual-byte accounting.
No nested archive is automatically extracted. Limit failures return
`release_limit_exceeded` with the limit name and safe observed count.

Cache fills coalesce by digest across threads and processes on one cache root.
Before downloading, the writer reserves space for compressed plus extracted
bytes within the cache budget. It evicts only unpinned, unused entries in
least-recently-used order. Active processes and candidate tests pin their trees.
If reservation fails, admission reports `insufficient_cache_space`; it must not
delete an in-use tree. Cancellation and crash recovery remove abandoned staging
directories after their fill leases expire. API-side temporary storage has the
same byte accounting and bounded fill concurrency.

## Pack index format 1

The initial canonical index stores immutable release history. It replaces the
single-current-version development catalog, without a dual-format reader:

```json
{
  "format_version": "1.0",
  "registry_name": "Attune Standard Pack Index",
  "registry_url": "https://packs.attune.io",
  "last_updated": "2026-09-16T00:00:00Z",
  "packs": [
    {
      "ref": "core",
      "label": "Core Pack",
      "description": "Core actions, triggers, and sensors",
      "author": "Attune",
      "license": "Apache-2.0",
      "keywords": ["core"],
      "channels": {
        "stable": "1.1.0"
      },
      "releases": [
        {
          "version": "1.1.0",
          "published_at": "2026-09-16T00:00:00Z",
          "requires_attune": ">=0.9.0,<1.0.0",
          "dependencies": {},
          "evidence": [],
          "archive": {
            "media_type": "application/vnd.attune.pack.v1+tar+gzip",
            "url": "https://packs.attune.io/core/1.1.0/core-1.1.0.attune-pack.tar.gz",
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "size": 16820341
          }
        }
      ]
    }
  ]
}
```

Resolution rules are:

- `core@1.1.0` resolves that exact version.
- `core@^1.1` resolves the highest compatible non-prerelease version.
- `core@stable` resolves the named channel.
- `core` resolves `stable`.
- A production install uses a canonical archive source.
- Failure to fetch or verify that archive fails closed. The client does not fall
  back to Git or a source snapshot for the same release.
- Registry, manifest, and `pack.yaml` ref and version values must agree.
- Index `requires_attune` and dependencies are resolution hints. Their values
  must match the authoritative manifest after download. A mismatch is a terminal
  `index_manifest_mismatch`, never a reason to try a source fallback.

All canonical publications use format 1, including binary-bearing releases.
The parser requires `format_version: "1.0"` and validates one strict schema.
Missing/unknown format versions, the superseded top-level `version` discriminator,
and mixed discriminators are errors. Do not silently adapt old development
catalogs; regenerate or explicitly convert them before cutover.
V1 includes author, license, keywords, last-updated time, and optional homepage,
repository, email, and use-case discovery fields. Optional per-release `contents`
and `runtime_deps` summarize the manifest for browsing; they do not authorize
execution. V1 uses channels and releases, not pack-level current version and
`install_sources`. These examples use placeholder sizes and digests.

### Dependencies and installation locks

The manifest's `dependencies` maps pack refs to semantic version constraints.
It must match `pack.yaml`'s required pack dependencies after normalization.
Missing dependencies default to an empty map. Optional dependencies are outside
the MVP activation contract. Registry selection comes from operator policy,
not a dependency's arbitrary URL. Resolution selects one version per pack,
intersects all constraints, and rejects cycles and incompatible ranges.

Before testing, the installer persists a complete lock containing each ref,
version, archive digest, selected registry and origin, compatibility requirements,
dependency edges, and expected active release ID. All dependency manifests must
be verified against the resolved graph. Retries reuse this lock without channel
resolution. Already active compatible dependencies are reused; changed nodes
are staged and tested together against the locked graph.

Changing a dependency also requires candidate tests for every affected required
dependent against the new graph, even when that dependent's own release does not
change. Its component projections need not be rewritten. The candidate evidence
must cover the prospective graph before any node can activate.

Activation is one database transaction for every changed node. It acquires one
repository-managed transaction advisory lock for the active dependency graph
before reading current graph state. Every activation, rollback, removal, and
required-lock update uses that same lock, including installs of new packs. This
prevents dependency write skew without holding a lock during downloads or tests.
The transaction then locks pack rows in ref order, checks expected active IDs
for changed and reused dependencies and tested dependents, and validates all
remaining active dependents before switching any node. Conflicts return
`activation_conflict` and require a new resolved install, not an implicit retry
against a changed graph. Object uploads occur before that transaction.

## Trust and evidence

### Required integrity

The MVP trust boundary requires:

- HTTPS for managed index and archive URLs.
- Existing outbound host and network policy checks.
- An index-pinned archive digest for registry installation.
- An embedded manifest with per-file digests.
- Exact ref and version agreement.
- An immutable version-to-digest binding.

An archive digest proves that downloaded bytes match the configured index. It
does not prove publisher identity if an attacker controls that index.

Local upload requires pack-install authorization and records the submitting
identity as the trust decision. An optional `--sha256` asserts expected bytes;
automated required-pack upload MUST supply the independently distributed lock
digest. The API computes and audits the actual digest in every case. Offline
verification proves structure and byte integrity, not publisher identity.

### Preserved supply-chain evidence

The embedded `evidence.sboms` and `evidence.provenance` arrays contain descriptors
with `path`, `media_type`, and `sha256`. Paths resolve to unique manifest-listed
files with the same digest. SBOMs may use SPDX or CycloneDX. Embedded in-toto or
SLSA statements describe source or payload artifacts by their file digests, not
the enclosing archive or manifest. Embedded signatures are not supported in v1.

Signatures and attestations whose subject is the release archive are detached.
Index release `evidence` entries have `kind`, `url`, `media_type`, `sha256`, `size`,
and `subject_sha256` equal to the archive digest. Allowed kinds are `signature`
and `provenance`. Evidence downloads use the archive outbound policy and a
16 MiB per-object limit. Evidence is stored separately by its digest and bound
to the immutable release, including for retention and offline export/import.
An authenticated upload may attach those same bytes without a URL.

Absent a configured signature verifier, the API reports `present_unverified`.
Failure to fetch optional evidence reports `unavailable` and does not claim
verification. Mandatory signature enforcement, key rotation, revocation, and
transparency-log policy remain a separate feature. Adding detached evidence
does not change release identity. Adding embedded evidence changes archive
bytes and therefore requires a new release version.

### Credentials

Index credentials and artifact-host credentials are separate secrets. Attune
MUST NOT forward index request headers to archive hosts by default. An operator
may configure host-bound credentials for a specific artifact origin.
All remote paths reuse the existing outbound client with redirects disabled,
DNS/IP validation, pinned resolved addresses, and proxy bypass. Credentials are
bound to scheme, host, and port and never copied from the index to another
origin. New artifact handling must not replace this client with a default HTTP
client. URL userinfo is rejected, and secret headers never enter audit records.

## Installation and activation

One install service owns resolution, fetching, validation, testing, publication,
and activation:

```rust
trait PackReleaseInstaller {
    async fn install(
        &self,
        request: InstallRequest,
    ) -> Result<InstalledRelease, InstallError>;
}
```

```rust
struct InstallRequest {
    source: InstallSource,
    idempotency_key: String,
    replacement_required_lock: Option<RequiredPackLock>,
    test_policy: TestPolicy,
    trust_policy: TrustPolicy,
}

enum InstallSource {
    Registry { requirement: PackRequirement, registry: RegistryId },
    Upload { staged_archive: UploadId, expected_sha256: Option<Sha256> },
}
```

The install operation follows this sequence:

1. Authorize the install and resolve its exact dependency lock.
2. Check Attune and platform-catalog compatibility.
3. Download each canonical archive to temporary storage.
4. Verify archive size and SHA-256 before extraction.
5. Validate archive structure, manifest, file hashes, modes, and component
   artifact references.
6. Store the exact archive bytes as an immutable release object.
7. Extract a private verified candidate tree.
8. Run required tests against the inactive candidate graph.
9. In one repository transaction, check the expected active graph, reconcile
   projections and ownership, apply the install's absent-metadata policy,
   persist sensor desired state, update active release IDs and any replacement
   required-pack lock, and insert audit and outbox records.
10. After commit, reconcile sensor processes and publish `pack.registered` from
    the durable outbox. Consumers deduplicate by outbox event ID.

There is no separately committed projection phase. A database failure rolls back
all changes. Failure before activation leaves the previous release active and
ready. A post-commit delivery failure retains the new active state and retries
the outbox; it is not reported as a rolled-back installation.

### Candidate tests and install states

Workers register using platform metadata before any pack is active. Candidate
execution uses the existing install-scoped candidate mechanism, extended to pin
the exact archive and dependency lock without repacking. Candidate sensor leases
and event routing are isolated by install ID. Their events can reach only test
rules and test executions, never production rules. Test keys, artifacts, and
inquiries are scoped to the install and cleaned up after retention. Tests receive
only operator-approved fixture credentials and endpoints, never automatic access
to production secrets. This does not sandbox arbitrary pack code; packs remain
trusted code and test fixtures must account for external side effects.

The persisted states are `resolving`, `fetching`, `validating`,
`waiting_for_capacity`, `testing`, `ready_to_activate`, `active_pending_delivery`,
`active`, `failed`, and `cancelled`. Successful processing follows that order,
with `waiting_for_capacity` optional. Pre-commit stages may terminate as failed
or cancelled. After commit, cancellation cannot undo activation. Delivery retry
exhaustion retains `active_pending_delivery` with an operator-visible error.

One lease owner advances an install; a monotonically increasing lease fence
guards each state write. Idempotency keys are scoped to the authenticated caller.
The same key and request return the existing operation; a different request with
the same key conflicts. Before activation, cancellation releases candidate pins
and schedules staging cleanup. Stored immutable releases may remain for GC.

Network timeouts, unavailable hosts, and transaction serialization failures are
retryable. Digest, schema, trust, test, and active-graph conflicts are terminal.
Defaults are five transient attempts with exponential backoff starting at two
seconds and capped at 30 seconds, bounded by a five-minute install deadline.
Capacity waiting consumes that deadline, not repeated test attempts. A cancelled
or failed operation requires a new key to restart. Resume verifies persisted
archive digests and reuses only protected, version-matched candidate trees.
Outbox delivery has a separate five-attempt automatic budget, followed by
explicit repair; it does not consume or reverse the committed install.

## Update and rollback

An update stages and validates the new release before activation. Existing work
continues to use its pinned release. New work uses the new active release.
Required smoke tests must pass before activation, including on first install.
Ordinary post-activation monitoring is not an installation test. A later runtime
failure degrades capability health and may require explicit rollback; activation
cannot undo external side effects or events already produced.

Attune MUST expose an audited rollback operation:

```sh
attune pack rollback core --to 1.1.0
```

Rollback activates a retained immutable release without downloading or
rebuilding it. It uses the activation transaction and reactivates stable component
IDs. The recorded dependency lock is restored only when all changed dependency
releases are retained and remaining active dependents stay compatible. Otherwise
rollback returns `rollback_dependency_conflict` before changing anything. A
rollback plan lists every affected pack and requires authorization for all of
them. It never upgrades or downloads dependencies implicitly.
When required content changes, rollback also stages the previously approved
required-pack lock and needs deployment-configuration authorization. The lock
and release graph switch in the same transaction; rollback cannot bypass pins.

Rollback preserves operator-owned permission assignments, policy associations,
and rule/queue references. It does not restore deleted external resources,
execution outcomes, or operator edits made since the earlier activation.
Retained test results may be reused only for matching release graph, platform
catalog revision, runtime versions, target, and worker image digest. Otherwise
candidate tests run from retained bytes. Offline rollback still requires local
workers and test fixtures, but no index or artifact-host access.

The activation audit records the old and new release graph, preserved component
IDs, test evidence, and initiating identity. Sensor desired-state changes use
fenced generations. Old generations lose event-ingress authority before a new
generation emits, even if the old process has not exited yet.

Retention MUST protect:

- The active release.
- Releases pinned by executions, enforcements, queue items, or sensor workloads.
- Releases and dependencies pinned by in-progress installs and rollback plans.
- The last two successful activation graphs and every successful activation
  within 30 days, by default. Both protections apply.

Object-store lifecycle rules MUST NOT expire blobs sooner than application
retention or bypass pins. Missing retained bytes make rollback unavailable and
raise an integrity alert rather than falling back to a download. Component
tombstones and operator relationships are not garbage-collected in MVP.

## Platform catalog and ownership

### Ownership model

Component namespace and lifecycle ownership are separate concepts.

```rust
enum ManagementOrigin {
    Platform { catalog_revision: u32 },
    Pack { pack_id: Id, release_id: Id },
    AdHoc,
}
```

Rules are:

- Platform metadata may retain `core.*` refs without belonging to the indexed
  `core` pack.
- Pack-owned refs still start with `<pack-ref>.`.
- A pack cannot declare or replace a platform-owned ref.
- Each install selects `remove`, `disable`, or `retain` as its
  `absent_metadata_policy`. The default is `remove`.
- `remove` retires omitted pack-owned metadata. `disable` keeps omitted
  non-toggleable metadata unchanged and blocks new work for omitted toggleable
  metadata. `retain` leaves omitted metadata unchanged.
- Present metadata always updates in place, reactivates, clears omission
  suppression, and records the incoming release.
- Omission suppression is separate from both the pack's declared `enabled`
  value and the operator-owned `enabled_override` value.
- Retirement preserves component IDs, content history, and operator-owned
  references. It does not issue cascading deletes. The same ref reactivates the
  same ID on rollback. Retired refs cannot be claimed by another owner.
- Platform reconciliation updates rows in place and preserves IDs.
- Ad hoc definitions remain outside pack omission cleanup.

This ownership distinction is required before the indexed core release omits
runtime and basic permission-set YAML.

Retired actions, triggers, rules, queues, sensors, and runtimes are ineligible
for new work. Permission resolution ignores retired permission sets while
retaining their assignments. Existing execution snapshots remain authoritative
for already-admitted work. All resolution and admission paths must apply this
state, not just list APIs. Pack-owned declarative fields revert on rollback;
operator-owned relationships and overrides remain separate and unchanged.

### Ownership migration

This is a coordinated maintenance migration, not a mixed-version rolling change.
Pause pack writes and stop old API, loader, bootstrap, and catalog writers before
backfilling origins. Drain or stop old execution services before enabling new
launch schemas. Transfer the exact built-in runtime, basic permission, and system
trigger refs to `Platform` in place, preserving IDs and assignments. All other
existing managed rows retain their pack owner. Conflicting ad hoc claims or
unexpected ownership abort migration for operator review; prefix alone never
authorizes takeover.

A database compatibility epoch prevents old service versions from restarting
against the new ownership schema. Before migration, revoke the old database
writer credentials and provision credentials only to compatible deployments;
an epoch field alone cannot fence binaries that never check it. Schema permissions
and deployment policy must prevent old bootstrap jobs from regaining write access.
Within a supported epoch, catalog reconciliation
uses a transaction lock and a monotonic revision check so an older replica cannot
downgrade definitions. The migration test must verify IDs, assignments, and
external references before and after the first external-core activation.

### Built-in platform metadata

The platform catalog contains metadata that Attune needs before pack
installation.

It MUST contain these basic permission sets:

- `core.admin`
- `core.editor`
- `core.executor`
- `core.viewer`

The reserved `standard` execution permission remains platform behavior rather
than an ordinary pack-defined permission set.

The platform catalog MUST contain the supported runtime catalog, initially:

- `core.shell`
- `core.python`
- `core.nodejs`
- `core.native`
- `core.java`
- `core.ruby`
- `core.perl`
- `core.go`
- `core.r`

The worker MUST register runtime name `native`, corresponding to database ref
`core.native`, alongside interpreter-backed runtimes.
Native availability cannot depend on the runtime registry being otherwise empty.

The platform catalog also contains:

- `core.alert` trigger metadata.
- `core.queue_started` trigger metadata.
- `core.queue_empty` trigger metadata.

Attune services emit those three trigger contracts directly. Their availability
must not depend on an optional content pack.

`core.key_creator` is not basic platform metadata. It belongs to the indexed
core release because its grants are specific to `core.generate_ssh_key_pair`.

## Indexed core release

### Required contents

The first external core release MUST include every current action ref:

- `core.echo`
- `core.noop`
- `core.sleep`
- `core.http_request`
- `core.generate_ssh_key_pair`
- `core.run_agent_command`
- `core.download_packs`
- `core.get_pack_dependencies`
- `core.build_pack_envs`
- `core.register_packs`

It MUST include `core.key_creator` while
`core.generate_ssh_key_pair` depends on that permission set.

It MUST include the timer content as one ownership unit:

- `core.timer_sensor`
- `core.intervaltimer`
- `core.crontimer`
- `core.datetimetimer`
- `core.rruletimer`
- A `timer_sensor` artifact with Linux `amd64` and `arm64` static variants.

It SHOULD include tests, documentation, examples, and any dashboards still
owned by the current core pack.

It MUST NOT redefine platform-owned runtimes, basic permission sets, intrinsic
handler implementations, or system event triggers.

### Core action cleanup decisions

The legacy pack-operation actions duplicate newer pack APIs and workflows. The
first external release keeps their refs to avoid combining distribution work
with removal work. A separate deprecation decision may remove them later.

The orphan legacy source `workflows/install_packs.yaml` is not discovered by the
current action loader and references missing `core.run_pack_tests`. The first
external release omits it and records that omission in its migration notes.
Restoring it later requires action metadata plus a separate `workflow_file`
graph and valid action refs, not copying the orphan unchanged.

### Core activation contracts

Core activation MUST verify:

- Every timer trigger named by `core.timer_sensor` exists in the same release.
- Every timer binary variant exists and passes manifest validation.
- At least one configured sensor host target matches a timer variant before the
  deployment reports timer capability ready.
- Required action runtimes exist in the platform catalog.
- Action-specific permission refs resolve.

## Clean-cluster bootstrap

The target startup flow is:

1. Database migrations run.
2. Attune reconciles the platform catalog in one idempotent transaction.
3. Bootstrap creates the initial identity and grants `core.admin` from the
   platform catalog.
4. The API starts without requiring an active pack release.
5. Action workers and sensor hosts register platform runtime, target, and cache
   capabilities without waiting for an active core release.
6. The bootstrap coordinator loads the operator's required-pack lock.
7. The coordinator stages exact releases through `PackReleaseInstaller` and runs
   isolated candidate smoke tests on those hosts.
8. The installer atomically activates the tested graph and reconciles workloads.
9. Deployment completion opens when content and required capabilities are ready.

The coordinator MUST use the same install service as the API. It MUST NOT call a
second Python loader or write pack component rows directly.

### Required-pack lock

Automated production bootstrap MUST pin required content independently of a
mutable index channel. The lock comes from the operator or tested Attune
distribution, not from the index fetched during bootstrap:

```yaml
required_packs:
  - ref: core
    version: 1.1.0
    archive_sha256: cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
dependency_pins: []
```

`dependency_pins` uses the same ref/version/digest fields and contains the full
transitive closure not already listed in `required_packs`. The example assumes
core has no pack dependencies. Missing, conflicting, or range-incompatible pins
fail before executing any candidate. The index may locate pinned bytes but must
not choose additional versions. Required-pack updates need an explicitly supplied
replacement lock. Ordinary installs cannot change a pinned dependency beneath
required content or make the deployed lock disagree with the active graph.
The replacement lock is staged on the install, not made authoritative during
testing. Its activation is atomic with the release graph. Until that commit,
readiness continues to use the previous active lock. Supplying a replacement
requires deployment-configuration authorization in addition to pack-install
authorization.

The Helm chart and Compose distribution MUST ship a tested lock that matches
the Attune application release. Operators may replace the index URL while
retaining the expected ref, version, and digest.

### Bootstrap test policy

First installation and updates use the same inactive-candidate test policy:

- Pre-activation checks validate the archive, component schemas, runtime refs,
  permissions, launch definitions, and sensor admission without execution.
- After platform-only hosts register, smoke tests run against the isolated
  candidate graph before activation. No provisional core release is exposed to
  production execution or sensor admission.

The standard distribution requires core including timer capability. An operator
may explicitly choose a platform-only profile with an empty required-pack lock.
That profile does not claim core action or timer readiness. Every coordinator
wait has a five-minute deadline and exposes its current stage on failure.

An emergency operator override is a separate audited operation. `force=true`
MUST NOT silently turn failed required-pack tests into a successful bootstrap.

## Health and readiness

Health states are distinct:

- Liveness means the process can serve requests.
- Platform readiness means the schema, database, platform catalog, and required
  control-plane dependencies are usable.
- Content readiness means every configured required pack has the locked active
  release and passed its required smoke tests.
- Capability readiness reports whether eligible workers exist for each required
  launch target.

`/health/live` is process liveness. `/health/ready` is platform readiness and is
the Kubernetes API readiness probe. `/health/content` reports required-content
and capability readiness separately. Existing `/health` remains a platform-only
health endpoint during deployment migration. No content failure changes liveness.

Internal API Services and worker startup use platform readiness only. They must
remain reachable while candidate tests or content repair run. Deployment tooling
checks `/health/content` after the control plane starts, with a five-minute wait.
An external gateway may gate user traffic on content health, but must not remove
the internal endpoints used for bootstrap, downloads, tokens, or tests.

Executor and worker process readiness permits platform-only startup. Per-pack
admission checks release synchronization, cache protection, and runtime
capabilities. Content readiness requires completed activation delivery and valid
test evidence. Capability readiness additionally requires live eligible hosts.
Capacity loss or a new untested worker image does not restart the control plane.

## Core smoke tests

The external core candidate passes only after these checks succeed in its
install-scoped test context:

- Execute `core.noop`.
- Execute `core.echo` with its declared dotenv input and verify exact text output.
- Execute `core.sleep` with a short duration.
- Exercise `core.http_request` against a controlled local endpoint.
- Verify `core.generate_ssh_key_pair` with a fixture-scoped equivalent of
  `core.key_creator`, and verify the declared permission ref separately.
- Run `core.run_agent_command` on a dedicated fixture host that exposes the
  configured executable. Existing `agent_mode`, `agent_binary_name`, and
  `agent_binary_version` are not proof that `attune-mcp` exists. If this capability
  is not required by the deployment, record `skipped_optional_capability` and do
  not advertise it as tested.
- Run source/schema checks for legacy pack-operation actions. Runtime checks
  require a disposable control-plane fixture and never modify the production
  installation. Record `static_only_legacy` rather than claiming runtime success.
- For a one-second interval rule, observe at least five distinct scheduled
  occurrences and five successful action executions within 30 seconds. Check
  occurrence timestamps, not delivery order. At-least-once delivery permits
  redelivery, which the test records rather than equating with a second owner.
- Stop the candidate timer process, allow lease recovery, and observe five more
  occurrences within 60 seconds. Assert at most one unexpired authoritative lease
  per workload and reject an event submitted using the old generation fence.

Timer, noop, echo, sleep, HTTP, inquiry, and key-creation checks cannot be skipped
in the standard profile. The fixture identity may access only test resources.
Candidate event scope and lease fences must be checked at ingress, not merely
encoded in a sensor's environment variables.

Results are keyed by release graph digests, catalog revision, target, runtime
versions, worker image digest or agent build digest, and test-suite digest. A
change to any key requires new candidate tests on that host class. A rolling
worker update retains the previously validated hosts until replacements pass;
it does not invalidate valid evidence for the old hosts.

## API and CLI changes

The CLI MUST provide:

```text
attune pack release build <pack-dir> --artifact <mapping> --output <archive>
attune pack release verify <archive>
attune pack release inspect <archive>
attune pack upload <archive> [--sha256 <digest>]
attune pack install <ref>@<version-or-range>
attune pack rollback <ref> --to <version-or-digest>
```

All commands support `--format json` for automation. `--output` on `release build`
is an archive destination, not an output-format selector. The build command also
accepts repeatable `--executable <pack-relative-path>` declarations for helpers.

`release build` MUST:

- Run source-level pack checks.
- Require every referenced logical artifact.
- Reject undeclared files under `.attune/artifacts/`.
- Compute file sizes, hashes, and modes.
- Generate the release manifest.
- Produce canonical archive bytes.
- Print the archive SHA-256.
- Avoid reading generated artifact paths through source ignore rules.

`release verify` MUST work offline.

The API MUST expose:

- Release inspection and evidence metadata.
- Exact-version installation.
- Installation status with bounded retry state.
- Active and retained releases.
- Audited rollback.
- Required-content readiness.
- Worker and sensor-host target capabilities.

## Failure behavior

The system fails closed in these cases:

- The archive digest differs from the index.
- Ref or version values disagree.
- A listed file is missing or has the wrong hash, size, or mode.
- An unlisted payload file exists.
- A component references an undeclared artifact.
- Native variants overlap. Missing hosts instead enter the bounded capacity wait
  defined by the install state machine or the execution scheduling deadline.
- A Java launch has an invalid main class or classpath outside the release.
- A pack attempts to replace platform-owned metadata.
- A version already maps to another digest.
- A required candidate smoke test fails.

Retries follow the install state machine. Status records the current stage,
attempt count, deadline, next attempt time, lock digest, and last safe error.
Post-commit repair never masquerades as a failed pre-activation update.

## Security requirements

- Archive extraction uses path-confined file creation and rejects links and
  special files.
- Validation enforces configurable limits for compressed bytes, extracted bytes,
  entry count, individual file size, manifest size, and artifact count.
- Upload, registry fetch, API storage, and worker extraction use the same limit
  policy.
- Pack files and all writable aliases are inaccessible for modification by child
  processes, as specified by cache protection. Chmod alone is insufficient.
- Native executable selection never uses an action parameter or untrusted path.
- Java arguments come from trusted runtime and pack metadata, not user input.
- New actions receive parameters and parameter secrets through stdin JSON.
  Existing delivery contracts remain readable for retained releases. Execution
  API tokens follow the existing permission-set opt-in environment contract.
- Release and evidence inspection never returns secret registry credentials.
- A system pack cannot bypass digest, ownership, or path validation.

## Module boundaries

| Module | Responsibility |
|---|---|
| `common::pack_format` | Manifest types, canonical paths, archive validation |
| `common::pack_registry` | Index parsing, version resolution, dependency planning |
| `common::pack_release` | Digest verification, storage, extraction, file verification |
| `common::platform_catalog` | Built-in runtimes, basic permissions, handlers, system triggers |
| `common::launch` | Typed launch validation and pure command planning |
| API pack installation | Authorization and install transaction orchestration |
| Pack component loader | Reconcile and retire verified pack-owned definitions without cascading deletes |
| Executor | Intrinsic dispatch and target-aware worker admission |
| Worker and sensor manager | Execute `CommandSpec` and verify pinned files |
| CLI | Build, verify, inspect, upload, install, and rollback commands |
| Bootstrap coordinator | Reconcile required pack locks and readiness |

The component loader does not parse archives, select native variants, seed
platform rows, or construct Java command lines.

## Compatibility

- No compatibility reader is required for superseded pre-production formats.
- Drain affected development workloads before resetting or explicitly converting
  old data; do not promise mixed-format execution during cutover.
- Never rewrite bytes under an existing digest. Canonical v1 executions and
  sensor workloads use pinned canonical releases.
- Existing `core.*` refs remain canonical.
- `entry_point` is an explicitly supported input for script, native, and Java
  source actions, not a legacy canonical-format compatibility reader.
- New JAR, class, intrinsic, and multi-target native launches require `launch`.
- One canonical index format 1 is supported; old development catalogs must be
  replaced or explicitly converted before use.
- Canonical binary-bearing publication uses the same format 1.
- Git and directory registration remain development inputs.
- Pack-prefix validation remains strict.
- Omission cleanup becomes management-origin aware before core content changes.

## Rollout plan

### Phase 1: platform ownership

- Add `ManagementOrigin` to pack-managed component types.
- Reconcile basic runtimes, roles, intrinsic handlers, and system triggers as
  platform metadata.
- Transfer existing built-in rows in place during the coordinated maintenance
  migration and enforce the service compatibility epoch.
- Replace destructive omission cleanup with origin-aware retirement and update
  every resolver to exclude retired components from new work.
- Fix normal worker registration of `native`.
- Split platform probes from content health and allow platform-only host startup.

### Phase 2: canonical archives

- Add the embedded release manifest.
- Add deterministic release build, verify, and inspect commands.
- Accept canonical archive upload without repacking.
- Apply manifest-declared executable intent on all ingestion and materialization
  paths.
- Provision protected caches, streaming verification, digest-coalesced fills,
  and shared resource limits before advertising format-1 execution capability.
- Add golden encoding vectors and resource-budget tests.
- Reject superseded development wire formats; document safe reset/re-import.

### Phase 3: typed launches

- Add `LaunchSpec` to action and sensor definitions and executable snapshots.
- Add native target capabilities to action assignment and fenced sensor leases.
- Add Java JAR and class launch planning.

### Phase 4: index and lifecycle

- Add canonical index format 1 release history and exact/range/channel resolution.
- Add dependency locking and Attune compatibility enforcement.
- Add isolated candidate action and sensor tests, persisted install states,
  atomic graph activation, transactional outbox delivery, and bounded retries.
- Add audited rollback with stable IDs and dependency-conflict checks.

### Phase 5: external core

- Create the external core pack repository or release workspace.
- Move all current core actions and timer definitions into it.
- Build both timer binary variants in external CI.
- Publish a canonical archive to the standard index.
- Ship an exact required-pack lock with Attune distributions.
- Add pre-activation candidate core smoke tests on platform-only hosts.

### Phase 6: remove direct bootstrap

- Replace `bootstrap_core_pack.py` and `load_core_pack.py` with the bootstrap
  coordinator and normal install service.
- Remove direct pack loading in `scripts/load-core-pack.sh`,
  `scripts/seed_core_pack.sql`, `docker/init-packs.sh`, and `docker/init-db.sh`.
  Update their Compose, image, CI, and Helm callers. Database/schema initialization
  stays separate from pack publication.
- Remove timer binary injection from init-pack images.
- Remove shared-volume core revision markers.
- Remove hard-coded core release startup waits.
- Remove superseded development-format readers; document explicit reset/re-import
  and drain requirements instead of a mixed-format compatibility window.

## Acceptance criteria

The feature is complete when all of these statements are true:

1. CI builds a pack archive containing an ignored native binary and an ignored
   JAR without copying either file into Git.
2. The supported encoding profile produces identical golden archive digests on
   Linux and macOS from identical source and artifact bytes, despite different
   source file modes, mtimes, and traversal order.
3. Archive upload, index installation, object storage, and worker download retain
   the same release digest.
4. Native intent survives canonical local upload, canonical index installation,
   legacy TAR/ZIP import, directory/Git canonicalization, API materialization,
   and shared-volume materialization. Each path has an explicit fixture.
5. A non-executable file never gains execute permission.
6. One installed release runs its `amd64` variant on an `amd64` worker and its
   `arm64` variant on an `arm64` worker.
7. Missing compatible capacity records `no_compatible_target` without dispatch.
8. Executable JAR and Java class actions run without shell wrappers.
9. A clean cluster creates its initial administrator before installing any pack.
10. A clean cluster installs the locked core release through the same installer
    used by `attune pack install`.
11. The indexed core release provides every current core action and the timer
    sensor.
12. The ownership migration and first external-core install preserve built-in
    IDs, permission assignments, and references; old writers cannot restart.
13. Timer restart satisfies the occurrence and lease-fence smoke checks. Repeat
    them across a tested core update, and reject emissions from the old generation.
14. Candidate failure, capacity timeout, activation conflict, or database failure
    leaves the previous core graph active and ready. No candidate event reaches
    a production rule. Post-commit delivery failure instead remains repairable.
15. Offline rollback restores retained release IDs and component IDs without
    losing permission assignments, policies, or rule/queue references. An
    incompatible dependent graph rejects rollback before any state changes.
16. Air-gapped installation from a local canonical archive passes the same
    validation as registry installation.
17. A worker child cannot modify, chmod, unlink, or replace a cached file or an
    ancestor directory. A host without cache protection is ineligible for v1.
18. Concurrent cold requests for one digest perform one fill per cache root.
    Warm launches perform no whole-archive read. Hashing memory does not grow
    with file size, and oversized input is rejected before exceeding its budget.
19. Boundary tests cover every resource limit, including dishonest TAR sizes,
    a nearly full cache, and restart with abandoned staging reservations.
20. Clean-cluster tests reach the internal API through its normal Service while
    content health is false. Candidate tests complete without readiness loops.
21. Crash injection before and after activation proves all-or-nothing graph
    changes, persisted delivery, idempotent replay, and bounded retries.
22. A detached signature can be attached and exported without changing archive
    bytes. Embedded self-hash and archive-subject signature constructions fail.
23. Racing installation of a dependent and an incompatible dependency update
    cannot both commit. A dependency-only update of required content needs a
    replacement distributed lock and fresh affected-dependent test evidence.

Performance verification records cold-fill duration, peak RSS, temporary disk
use, and warm-launch p50/p95 on both supported architectures. Run at one and 20
concurrent starts with 16 MiB, 128 MiB, and 512 MiB compressed fixtures, including
two-target native packs and self-contained JARs within the extracted-byte limit.
Compare against the legacy path on the same hosts. The release gate requires
one digest fill, bounded buffers, no warm whole-archive scan, and no staging
budget overflow. Measurements determine later latency targets; the spec does
not invent throughput guarantees before those measurements exist.

## Resolved design decisions

These defaults resolve the design review. Implementations follow them unless a
subsequent specification revision changes the contract.

| Decision | Choice | Why it matters |
|---|---|---|
| Release packaging | One universal archive | Keeps one release ID and digest across mixed workers |
| Initial native targets | Linux `amd64` and `arm64`, static | Matches current deployment builds and avoids libc matching |
| Published source | Canonical archive | Git cannot represent generated files absent from the repository |
| Version history | Index format 1 release list | Exact versions must remain installable after a channel advances |
| Executable intent | Embedded manifest | TAR, ZIP, Git, and Windows modes are not a stable contract |
| Core refs | Preserve `core.*` | Avoids workflow, rule, API, and permission migration |
| Core publication unit | One indexed `core` pack | Avoids version skew across basic functions and timer content |
| Platform metadata | Separate catalog with explicit ownership | Breaks bootstrap and cleanup cycles |
| System triggers | Keep alert and queue triggers built in | Platform services emit them before optional packs |
| Action-specific permissions | Keep in indexed core | They should release with the action they constrain |
| Core bootstrap version | Exact version and digest lock | A mutable channel is unsafe during cluster startup |
| First-install and update tests | Isolated inactive candidates on platform-only hosts | Prevents untested production activation |
| JAR distribution | Self-contained JAR for MVP | Avoids Maven resolution and mutable dependency downloads |
| Java classpath | Explicit ordered entries, no globs | Keeps command construction deterministic |
| Signature enforcement | Detached archive evidence, mandatory independent core lock | Avoids recursive signing and index-only bootstrap trust |
| Private source auth | Host-bound artifact credentials | Prevents index credentials leaking to another origin |
| Required content health | Platform-only internal readiness, separate content gate | Keeps bootstrap and repair API calls reachable |
| Omitted components | Per-install `remove`, `disable`, or `retain` policy | Preserves stable IDs and operator overrides while letting deployments choose admission behavior |
| Ownership migration | Coordinated maintenance with compatibility epoch | Prevents old loaders deleting newly platform-owned rows |
| Cache integrity | Protected execution view plus streamed verification | Owner-writable modes and markers are not protection |
| Dependency activation | Locked graph and one transaction | Prevents partial upgrades and implicit re-resolution |
| Rollback retention | Last two successful graphs and 30 days | Defines a usable default without overriding live pins |
| Legacy pack-operation actions | Ship once, then decide deprecation separately | Keeps this migration focused on distribution and bootstrap |
| Universal archive size threshold | Measure before adding target layers | Layered releases add identity and recovery complexity |

These release-management questions remain outside the resolved contracts:

- Which Attune application version first ships canonical format 1 and external core?
- How long do format 1 index and legacy `entry_point` publication remain accepted?
- Which trust-root format and signature policy will protect the standard index
  and core releases?
- Should a future release split privileged pack-operation actions from low-risk
  utility actions after the initial externalization succeeds?

## Design rationale

Three designs were compared.

The selected design uses one canonical archive, preserves `core.*` refs, and
separates platform metadata ownership from pack ownership. It gives authors one
artifact, operators one installed release, and workers one deterministic target
selection rule.

A split-pack design moved utilities, key tools, pack operations, inquiry, and
timers into separate namespaces. It reduced each pack's scope but introduced
version skew, dependency ordering, aliases, and a broad ref migration. Source
code may use those internal boundaries without exposing them as publication
units.

A layered-bundle design stored definitions and each target variant as separate
objects. It reduced worker downloads but added several digests, partial download
states, overlay materialization, and more recovery rules. This optimization is
deferred until archive size data shows that universal archives are a problem.

The chosen interface hides archive normalization, checksums, target selection,
Java launch details, ownership cleanup, and activation behind the release
builder, installer, and launch planner. Pack authors declare what a component
needs to run. They do not coordinate those internal stages themselves.

## Current implementation references

The implementation work should start from these files:

- `crates/common/src/pack_registry/archive.rs`
- `crates/common/src/pack_registry/storage.rs`
- `crates/common/src/pack_registry/loader.rs`
- `crates/common/src/pack_registry/mod.rs`
- `crates/common/src/pack_transport/api.rs`
- `crates/common/src/pack_transport/volume.rs`
- `crates/common/src/pack_registry/outbound.rs`
- `crates/common/src/models.rs`
- `crates/api/src/routes/packs.rs`
- `crates/api/src/routes/health.rs`
- `crates/worker/src/runtime/process.rs`
- `crates/worker/src/runtime/native.rs`
- `crates/worker/src/service.rs`
- `crates/sensor/src/sensor_manager.rs`
- `crates/executor/src/scheduler.rs`
- `crates/executor/src/work_queue_events.rs`
- `crates/common/src/system_alert.rs`
- `crates/core-timer-sensor/`
- `packs/core/`
- `scripts/bootstrap_core_pack.py`
- `scripts/load_core_pack.py`
- `scripts/load-core-pack.sh`
- `scripts/seed_core_pack.sql`
- `docker/init-packs.sh`
- `docker/init-db.sh`
- `../attune-charts/charts/attune/templates/applications.yaml`
- `../attune-charts/charts/attune/templates/jobs.yaml`
