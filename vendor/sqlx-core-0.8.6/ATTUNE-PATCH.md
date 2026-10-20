# Attune SQLx core patch

This directory vendors the published `sqlx-core` **0.8.6** crate, not a new SQLx
release. The local patch has not been accepted upstream. The published package
version, features and dependency declarations remain unchanged.

## Upstream provenance

* Archive: <https://static.crates.io/crates/sqlx-core/sqlx-core-0.8.6.crate>
* Archive SHA-256: `ee6798b1838b6a0f69c007c133b8df5866302197e404e8b6ee8ed3e3a5e68dc6`
* Upstream repository: <https://github.com/launchbadge/sqlx>
* Recorded upstream commit: `bab1b022bd56a64f9a08b46b36b97c5cff19d77e`
* Applied driver proposal SHA-256: `645a6c37acf7c635c623c3fbd237d785467da925c78ab100b355202d6a20905f`

`LICENSE-MIT`, `LICENSE-APACHE`, `.cargo_vcs_info.json`, `Cargo.toml` and
`Cargo.toml.orig` are preserved byte-for-byte from the archive. Both upstream
MIT and Apache-2.0 licenses remain available verbatim. Other than the maintained
source files below and this note, archive members are unchanged. Only the
maintained Rust files were formatted after applying the reviewed proposal.
The compatibility maintenance below is additional to that driver proposal.

## Maintained source and purpose

* `src/net/socket/mod.rs`: adds a default `WithSocket::with_tcp_socket` callback
  to capture the connected TCP peer before TLS wrapping or type erasure. Adds
  `connect_tcp_addr`, sharing the existing runtime selection and socket setup.
  Tokio and async-std receive the complete captured `SocketAddr` directly,
  including IPv6 scope ID and flow information. The existing hostname entry
  point and Unix-domain socket support remain available.
* `src/net/mod.rs`: re-exports `connect_tcp_addr` for the paired PostgreSQL patch.
* `src/pool/inner.rs`: retains the existing `fetch_update` call, orderings and
  pool-capacity closure. A single let-statement `allow(deprecated)` with a reason
  permits that API on Rust 1.99 while preserving supported Docker Rust 1.98.1.
  Replacing it with the new `try_update` name would break that supported compiler.
* `src/migrate/migration.rs`: copies the same SHA-384 digest bytes with `to_vec`
  instead of deprecated GenericArray `as_slice`. The checksum algorithm and SQL
  input are unchanged; no cryptography dependency is upgraded.
* `src/query.rs`, `src/query_as.rs`, `src/query_scalar.rs`: spell the already-bound
  statement lifetime as `Arguments<'q>` in the three `query_statement` return
  types instead of inconsistently eliding it as `Arguments<'_>`.

These compatibility changes address warnings exposed by building the crate as a
local path dependency. They add no crate-wide or function-wide lint suppression,
dependency changes or compiler-version bump. Only the one atomic statement has a
deprecation exemption; unrelated uses remain checked. No Rust 1.98.1 or Rust 1.99
build result is claimed by this provenance note.

The paired `../sqlx-postgres-0.8.6/ATTUNE-PATCH.md` explains the opaque PostgreSQL
CancelRequest capability. Its cancellation channel uses the original TLS options
and certificate hostname while connecting to the captured full TCP endpoint.
It does not re-resolve a hostname or replace TLS identity with a peer IP string.
The capability adds no public raw backend PID/secret constructors, getters or
serialization, and its formatting is redacted.

PostgreSQL protocol 3.0 supplies a four-byte backend cancellation key. The paired
driver accepts only that negotiated shape. Cancellation-channel EOF is transport
completion, not a backend acknowledgement of cancellation or rollback. The
application owns the bounded channel permit and deadline and must independently
observe the target operation settling before allowing its session to be reused.

## Regression coverage to retain

`src/net/socket/mod.rs::exact_tcp_target_preserves_scoped_ipv6_socket_addr`
checks the exact-address adapter with a scoped IPv6 address and nonzero flow
information without opening a socket. The paired driver tests packet encoding,
format redaction and unsupported backend-key lengths. These source tests do not
by themselves prove live cancellation routing, TLS behavior or proxy support.
No upstream acceptance or runtime-validation claim is made by this note.
