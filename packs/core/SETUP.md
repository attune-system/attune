# Core setup

Core is published by the normal authenticated init-packs API bootstrap. Configure
a registrar token and a compatible published core image/version in deployment
configuration. This source targets core 1.0.9; its publication is a separate
release step. Do not assume the source version means an image already exists.
Core 1.0.8's HTTP action supports only public destinations; internal automation
calls formerly supported by 1.0.7 require a separately reviewed operator policy.
The token does not grant action execution authority. Runtime actions need only the
native capability, which the worker advertises even in interpreter-limited pools.

Compose builds static pack binaries with `docker/Dockerfile.pack-binaries` and
assembles its host variant in a private publication candidate. Official release
images contain AMD64 and ARM64 action variants plus the existing timer binaries.
There is no source/interpreter fallback. See [packaging details](README.md).

Upgrades use the existing pack-wide omission removal policy. The release keeps all
nonaction definitions and removes the seven obsolete action definitions plus
the installer action's owned workflow graph/definition. Existing
rows and historical release pins are retained under the ordinary lifecycle rules;
no database reset or manual cleanup is part of this change.
