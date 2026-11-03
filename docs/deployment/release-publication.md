# Release publication

`.github/workflows/publish.yml` keeps server/container publication independent
of optional CLI distribution outputs.

Required inputs remain the release-commit CI gate, metadata, Linux runtime
binary bundles, bundled core sensor binaries, Docker distribution, and OpenAPI
specification. Container builds and manifest publication are not optional.

Optional outputs are Windows CLI archives, signed and notarized macOS CLI
archives, standalone Linux CLI archives, and Linux OS packages. Homebrew, the
Arch CLI repository, and Nexus package publication are also non-blocking.
Failures remain visible in their jobs; the release summary reports which
artifacts are available.

Release assembly uses uploaded artifacts rather than a tolerated job result as
proof that an optional output exists. It publishes available CLI archives only
with their checksum files. macOS staging artifacts are never release assets.
Homebrew updates require both notarized macOS architectures. Arch CLI repository
updates require both Linux CLI archives. Linux OS packages are included only
when both architectures and their Arch signatures are available; signature
verification remains required before those packages are published.

The Linux `attune` and `attune-mcp` binaries embedded in the agent image remain
required runtime inputs. Optional standalone packaging does not permit shipping
an incomplete agent container. Pull-request Windows CLI validation remains a
normal failing check; optional release packaging does not suppress build defects.
