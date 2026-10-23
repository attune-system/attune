# Swagger UI security patch

Source is the crates.io `utoipa-swagger-ui` 9.0.2 release, including its original
MIT and Apache licenses. The library's Rust sources are unchanged.

Local changes to the published Cargo manifest:

- Upgrade the build dependency `zip` from 3.0 to 9.0 to fix
  [RUSTSEC-2026-0369](https://rustsec.org/advisories/RUSTSEC-2026-0369).
- Set the minimum Rust version to 1.88, as required by `zip` 9.

`build.rs` also propagates the now-fallible `ZipFile::name()` result with `?`.

Swagger UI 10.0.1 still requires vulnerable `zip` 8.6 and also changes the
`utoipa` major version. The workspace patch preserves the existing OpenAPI API
without an advisory exception. Remove it when a compatible upstream release
uses a patched `zip` version.

The upstream lockfile and Cargo registry bookkeeping files are not vendored.
