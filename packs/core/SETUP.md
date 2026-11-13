# Core setup

Core is published by the normal authenticated init-packs API bootstrap. Configure
a registrar token and the core 1.0.7 image/version in the deployment configuration.
The token does not grant action execution authority. Runtime actions need only the
native capability, which the worker advertises even in interpreter-limited pools.

Compose builds static pack binaries with `docker/Dockerfile.pack-binaries` and
assembles its host variant in a private publication candidate. Official release
images contain AMD64 and ARM64 action variants plus the existing timer binaries.
There is no source/interpreter fallback. See [packaging details](README.md).

Upgrades use the existing pack-wide omission removal policy. The release keeps all
nonaction definitions and removes the eight obsolete action definitions. Existing
rows and historical release pins are retained under the ordinary lifecycle rules;
no database reset or manual cleanup is part of this change.
