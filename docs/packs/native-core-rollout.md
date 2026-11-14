# Native core 1.0.7 rollout gate

Core 1.0.7 contains only native echo, sleep, noop, and HTTP request actions. Their directory
entrypoints require updated worker/agent code. Stock platform 0.10.4 workers
advertise `native` but cannot dispatch those directories. An app-version string
or advertised native capability alone is not compatibility evidence.

The implementation code floor for this rollout is
[`c462efe6e5bc36449fe79c5010345ed92bcbadfe`](https://github.com/attune-system/attune/commit/c462efe6e5bc36449fe79c5010345ed92bcbadfe),
or a descendant retaining its directory dispatch and confinement behavior.
Deployment additionally requires successful native/archive/admission validation
and an approved worker/agent image built from compatible integrated code. This
source reference does not assert that an image has been released or approved.
In particular, preserve the execution-environment Result propagation when
integrating the independent environment remediation.

## Stage workers before activating core

1. Build and validate the integrated worker/agent artifacts. Verify their immutable
   image provenance and architecture against the approved implementation revision;
   do not invent a tag or infer compatibility from `0.10.4` metadata.
2. Replace worker/agent pools while the currently admitted old core remains active.
   Ordinary native file entrypoints retain their previous execution path. Retain
   any interpreter packages needed by that old release during this stage.
3. Verify the intended pools run compatible binaries, drain/replace incompatible
   workers, and run the owned native-only upload/preflight and archive execution
   checks. No new synthetic capability label substitutes for these checks.
4. Only after that barrier, authorize normal publication/activation of core 1.0.7.
   Official archives must include both static AMD64 and ARM64 variants. Local
   Compose host-only assembly is explicitly separate from official publication.

A chart pre-upgrade hook runs before ordinary deployment replacement. It must not
activate 1.0.7 while old worker pods are still serving. Use the staged worker
replacement under old core first, or a separately authorized coordinated app
release. Chart version/core pin updates remain draft until a compatible image and
safe hook ordering exist. This PR performs no deployment, release, or image-tag
substitution.

## Preserve release and omission semantics

Bootstrap uses the existing scalar **pack-wide** `absent_metadata_policy=remove`.
The candidate retains all unrelated nonaction definitions byte-for-byte. Eight
omitted action rows and the removed installer's owned workflow definition retire;
unrelated sensor/runtime/IAM/dashboard definitions and operator relationships are
not intentionally changed. Normal projection bookkeeping may update during
admission. There is no action-only mask or new API policy.

Retirement prevents new lookup/admission through the removed refs. Historical
rows and already-admitted immutable release pins remain available under existing
retention and authority rules. Do not reset a database, erase retained releases,
rewrite a version-to-digest binding, or strip old runtime prerequisites while old
pinned work still needs them.

Local Compose stages compiled inputs in `pack_binaries`, a separate volume mounted
read-only by bootstrap and absent from workers. It assembles a private host-only
candidate and never writes compiled updates into the active core projection.
See [the core packaging contract](../../packs/core/README.md) and
[its validation commands](../../packs/core/TESTING.md).

The optional legacy storage producer using the removed agent-command action is
explicitly deferred coverage in local issue #142. Its dispatch/provisioning path
fails before creating a replacement authority. Shared API/private helpers and the
independent native-controller fixture remain supported; source/mock checks do
not certify a live controller acceptance run.
