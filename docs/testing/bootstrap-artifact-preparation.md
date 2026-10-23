# Native-inclusive acceptance artifact preparation

This source-ready unit closes the missing reproducible build recipe/input path.
It does not turn an offline resource graph, accepted PG tests or an OCI receipt
into live deployment acceptance. No builds or generation jobs ran while writing it.

## Default plan and central-only execution

```sh
python3 scripts/prepare-bootstrap-artifacts.py
```

Default operation reads the example manifest and prints commands only. It creates
no work root or subprocess. The example contains unresolved prerequisites, not
invented image/archive digests. Central execution requires both explicit flags:

```sh
python3 scripts/prepare-bootstrap-artifacts.py \
  --manifest /owned/acceptance/build.json \
  --work-root /owned/acceptance/build-unique123 \
  --execute --permit-build
```

Use `scripts/acceptance/artifact-build.example.json` as the input shape. Supply:

- A frozen source tar archive, its actual measured SHA256, real integrated source
  revision and fixed `source_date_epoch`. The archive contains the current combined
  code, migrations, SQLx metadata, workspace and standalone timer Cargo locks,
  current SDK-generated source, acceptance recipes and F inspector example.
  No moving primary checkout, prerequisite worktree snapshots or unreviewed changes
  are silently overlaid. A reviewed staged combined snapshot may differ from HEAD;
  its separate archive hash is the byte-level provenance, not just a revision label.
- An explicit local Docker context and existing central permissioned BuildKit
  builder slot. The script neither creates nor prunes that builder. Its scheduler
  must enforce at most two build jobs and serialize this job with other heavy work.
  Preflight uses Buildx's JSON inspect format and requires exactly one already
  running docker/docker-container node attached to that validated local context.
  A local Docker context cannot authorize a remote/Kubernetes or inactive builder.
  Run on the selected native Linux amd64 or arm64 host. No QEMU fallback is inferred.
- Seven real digest-pinned base images. The toolchain contains a compatible Rust,
  cargo-zigbuild, Zig, native linker/SSL development dependencies and installed
  x86_64/aarch64 musl targets. The Node 24 image supplies npm. The compatible service
  runtime base supplies glibc/libssl, CA roots, curl/git and the attune UID 1000
  account. The Python base already contains PyYAML. BusyBox, PG18 client and Nginx
  bases complete packaging. Pin these curated build/runtime inputs by actual
  digest; the recipe does not install unpinned packages or toolchains. Host-run
  inspector/helper binaries require the matching host ABI/runtime libraries too.

One way for the integration owner to capture committed source after the freeze:

```sh
git -C /owned/integrated archive --format=tar --output=/owned/source/integrated-source.tar ACTUAL_REVISION
sha256sum /owned/source/integrated-source.tar
git -C /owned/integrated show -s --format=%ct ACTUAL_REVISION
```

For staged/uncommitted combined source, use the integration owner's reviewed
snapshot procedure and hash its actual output instead. Do not commit primary to
manufacture a build revision or export a stale HEAD while claiming staged fixes.
Source tar members are bounded and must be ordinary files/directories: links,
traversal, operational env/private files and caches are rejected. Keep any source
archive credentials out entirely, not merely hidden by a Docker ignore pattern.

## What actually runs later

The fixed sequence is one foreground BuildKit invocation at a time:

1. Build real service binaries, actual archive/inspector examples, injected agent/
   sensor/CLI/MCP binaries and both timer architectures from the frozen source.
   All Cargo/zigbuild commands use `--locked -j 2` and SQLx offline metadata. No
   placeholder main functions, DB connection, test execution or lock regeneration.
2. Invoke the real canonical `PackStorage` archive helper inside that build using
   the assembled core tree. The helper checks both executable native ELF inputs.
   Host collection independently hashes its actual canonical `pack.tar.gz`.
3. Export OpenAPI through the current offline exporter. Build/generate the web
   client only inside the isolated build image using that exact API document and
   npm's lockfile. No primary SDK/web/generated files are modified.
4. Package API, executor, notifier, supervisor, injected agent, native-inclusive
   init-packs, migrations and web into eight local OCI tar archives. Source and
   fixed-time inputs are recorded. Init-packs uses an API-only offline-dependency
   entrypoint: no runtime pip, old flat-binary adoption or mutable projection copy.

The script verifies exported OCI blob hashes, root descriptor and referenced
content sizes. It measures each OCI tar checksum separately from the actual image
manifest digest. Its receipt also records actual host-native binary checksums and
the canonical core archive version/digest/path. No digest is printed by a plan.
Pinned inputs and fixed timestamps improve repeatability; this is not a claim of
bit-identical independent rebuilds without comparing their actual receipts.

## Outputs and bridge to acceptance

`artifact-receipt.json` is `BUILT_NOT_DEPLOYED` only after every required output
passes. It contains `core`, `binaries`, `images` and completed-stage inventory.
Use actual `binaries.bootstrap_acceptance_archive` and
`binaries.storage_probe_inspector` paths/hashes in the lifecycle manifest. The
separately precompiled F repository test binary is still a central test-lane input;
this build unit does not rerun or silently replace that test inventory.

OCI archives are not automatically daemon repository references. The integration
owner must explicitly authorize/import them into an owned registry, preserving or
remeasuring manifest digests. For an already owned loopback registry, a central
operator can use a tool such as:

```sh
skopeo copy --preserve-digests --dest-tls-verify=false \
  oci-archive:/owned/acceptance/build-unique123/api.oci.tar \
  docker://127.0.0.1:OWNED_PORT/eaccept/UNIQUE_RUN/api:current
docker --context OWNED_LOCAL_CONTEXT pull \
  127.0.0.1:OWNED_PORT/eaccept/UNIQUE_RUN/api@sha256:ACTUAL_MEASURED_MANIFEST_DIGEST
```

Those are later explicit owner commands, not actions performed by this script.
Never use a production registry, weaken TLS for a remote registry or assume an
archive checksum is a manifest digest. If an import changes representation, measure
the actual imported digest rather than copying the old value. Registry provisioning
and teardown need their own explicit ownership; this unit does not launch one.

Feed actual references into the Docker repository's `acceptance-images.compose.yaml`
and Helm `images.<component>.digest` inputs. Keep the core archive digest separate.
Neither packaging path changes the existing unreviewed Docker web dependency.
The lifecycle runner still requires both profiles, ordinary provided first-operator
credentials, separate installer/invoker/local-password maintenance principals,
real F inspector/observer and native legacy-writer/controller bindings. Missing
images or incompatible/gate-OFF readiness fail instead of counting denial as PASS.

## Coverage and still-required live work

| Requirement | Evidence/status from this unit |
| --- | --- |
| Actual native-inclusive archive and compatible image inputs | Runnable central build/measurement source, not built here |
| Fresh all-component deployment, actual worker/sensor/UI startup | Existing owned lifecycle source; central live matrix still required |
| Failed upgrade, unchanged old pins, committed outcome and retained rollback | Existing real PG/API fixtures and runner assertions; do not replace uncertain HTTP outcomes with retries |
| Old-image readiness skew | Existing actual-image probe; real incompatible image remains required |
| SharedFile and ObjectSegments physical lifecycle | Concrete F probe sender and distinct principals; central provider execution required |
| Preexisting file-evidence upgrade/native request settlement | F owns the real old-writer/controller implementation and proof; no synthetic legacy rows or settlement flags supplied here |
| Helm fresh/failed-required-upgrade/rollback in both modes | Separate external disposable cluster/context and owned namespace/PVC matrix; offline render is not acceptance |

Coordinate new native settlement inputs through F's existing binding protocol,
not another controller or a new C reader. Current source/PG successes are retained
as prior evidence, not rerun by this unit and not recast as Compose acceptance.

## Cleanup and failure

The work root must be new. It remains mode 0700 with mode-0600 receipts and is
retained deliberately, including incomplete output on failure. There is no recursive
delete, volume reset, image prune or adoption of old paths. No application/provider/
DB container, network, volume or cluster is created. Build children run in joined
foreground groups using the existing bounded command owner. On cancellation the
client is terminated/awaited and the receipt remains INCOMPLETE. The central
scheduler owns the pre-existing builder and must confirm remote BuildKit work is
drained before reusing its slot; the script never deletes someone else's cache.
No DB/provider volume is at risk while collecting these filesystem artifacts.

Compilation, image imports and service lifecycle are intentionally separate
permissions. The source release gates are compiled unchanged; no build receipt
claims that admission is enabled or the full user SOW has passed.
