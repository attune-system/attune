# Offline format-1 release commands

`attune pack release build`, `verify`, and `inspect` operate without a profile,
authentication, or an API connection. They do not execute pack content.
Inspection performs the same verification as `verify` before returning metadata.

## Commands

```sh
attune pack release build ./acme \
  --artifact 'tool[linux/amd64/static]=dist/tool' \
  --artifact 'app=build/app.jar' \
  --executable helpers/check \
  --output dist/acme.attune-pack.tar.gz --format json
attune pack release verify dist/acme.attune-pack.tar.gz --format json
attune pack release inspect dist/acme.attune-pack.tar.gz --format json
```

`--output` on `build` is a required archive path. It never overwrites an existing
file. `--format json` selects result serialization independently of that path.
Root `--output json` and `--json` also select JSON for build results without
conflicting with the local archive destination. Other commands retain their
existing `--output` behavior.

Artifact mappings are repeatable. Native mappings require `linux/amd64/static`
or `linux/arm64/static`. A mapping without a target supplies a portable JAR.
Paths on the right of `=` resolve relative to the CLI's working directory.
Explicit artifacts bypass source ignore rules.

Source collection honors pack-local `.gitignore` and `.ignore` files. It ignores
global and parent ignore configuration so another user's Git settings cannot
change the release. VCS directories are omitted. Links and special files fail
validation. Source files cannot claim `.attune/artifacts/` or the manifest path.
Source filenames are normalized to NFC before collision checks. Canonical
archive verification rejects non-NFC paths rather than rewriting them.

Native variants and legacy native entry points receive executable intent.
`--executable` declares other helper programs. Local mode bits never grant
execute permission. Definitions, interpreter inputs, and Java inputs remain data.

JSON results contain `manifest`, `sha256`, `compressed_size`, `extracted_size`,
`largest_file`, and `entry_count`. Sizes and counts include the manifest where
specified by the release format. Failures exit nonzero and print diagnostics to
stderr without a success JSON document.

## Shared interface

`attune_common::pack_format` exports these blocking operations:

```rust,ignore
build(&BuildOptions, &Path, Limits) -> anyhow::Result<VerifiedRelease>
verify(&Path, Limits) -> anyhow::Result<VerifiedRelease>
inspect(&Path, Limits) -> anyhow::Result<VerifiedRelease>
```

`BuildOptions` contains a source directory, typed `ArtifactInput` mappings, and
helper executable paths. `Limits` supplies all seven format-level budgets.
`VerifiedRelease::manifest()` exposes the strict manifest after verification.
Launches, classpath entries, artifact kinds, and native target values use enums.
The CLI runs these operations through `spawn_blocking`.

The builder snapshots files in private temporary storage. Verification streams
hashes with 64 KiB buffers and creates a private temporary tree for source checks
and seek-based JAR inspection. The encoder uses a temporary uncompressed TAR.
Temporary disk use is bounded by input budgets but is not a cache reservation.

`pack.yaml` may provide `requires`, `source`, and `evidence` with the corresponding
manifest shapes. Absent requirements default to Attune `*`, platform catalog
`>=1`, and normalized `runtime_deps`. Source `dependencies` uses the existing
array of `ref@constraint` strings. The manifest records normalized constraints.

## Encoding profile

The manifest schema is `attune.pack.release/v1` and the media type is
`application/vnd.attune.pack.v1+tar+gzip`. There is no v2 or development-format
wire reader. Pack release versions are independent of the format identifier.

The profile is `ustar-miniz_oxide-0.9.1-level9-unicode15.1-v1`. The encoder calls
pinned `miniz_oxide` directly, independent of `flate2` backend feature selection.
Pinned ICU4X 1.5 data supplies Unicode 15.1 normalization and full case folding.
`serde_jcs` provides RFC 8785 manifest serialization.

The generated golden fixture contains both native targets, an executable JAR,
a class directory, an explicitly executable helper, and a decomposed Unicode
source filename. Its measured values are:

| Value | Expected |
|---|---|
| Archive SHA-256 | `46c80ccd915d412f051175384522d50f9bbc489bbea31ce7fe1a5dc0a4986761` |
| Compressed bytes | 1865 |
| Payload plus manifest bytes | 3188 |

`cargo test --locked -p attune-common --lib pack_format` recreates the fixture,
changes source traversal order, modes, and mtimes, then compares exact bytes.
`.github/workflows/pack-format-golden.yml` runs this test on Linux and macOS.
Format-1 publication remains gated on both platforms passing.

## Current boundaries

- This module does not publish, install, extract into a persistent cache, plan
  processes, or modify platform catalog ownership.
- Directories are the supported development/source input. Native `entry_point`
  declarations and explicit helper declarations provide executable intent.
  TAR, ZIP, and Git source imports are not part of this CLI's input contract.
- JAR `Class-Path` attributes with nonempty values are rejected, including
  release-local values. Dependencies must use explicit classpath artifacts.
- ZIP64, split JARs, and self-extracting ZIP prefixes are rejected. Stored and
  DEFLATE entries, data descriptors, and ZIP comments are supported.
- ZIP comments cannot contain the EOCD marker `PK\x05\x06`, even when it does
  not form a complete end record. This excludes alternate directories that a
  JVM might select despite trailing bytes after their end record.
- The JAR reader parses one exact central directory under the manifest byte
  budget. It rejects ambiguous end records, gaps, overlaps, mismatched raw local
  and central names, and Unicode Path extra fields. It never falls back to an
  earlier directory or lets an extra field rename the JVM's manifest.
- Every physical JAR manifest line must end with LF, CRLF, or CR. Before
  unfolding continuations, the reader enforces a conservative maximum of 72
  UTF-8 bytes per physical line, excluding the terminator but including any
  continuation space. Longer values must use continuation lines.
- JVM options accept self-contained `-D`, `-XX:`, `-Xmx`, `-Xms`, assertion, and
  client/server options. Launch replacement options and argument files fail.
- `file` launches accept known built-in interpreter runtimes. Offline validation
  does not resolve custom runtime definitions.
- Metadata files have the existing source checker's 1 MiB bound. Its traversal
  and metadata-count limits also apply, even if archive limits are higher.
- SBOM descriptors are checked against payload paths and hashes, without an
  SPDX or CycloneDX semantic validator. Nonempty embedded provenance is rejected
  until file-subject validation can exclude archive-subject statements.
  Detached signatures and publisher authenticity are not implemented.
