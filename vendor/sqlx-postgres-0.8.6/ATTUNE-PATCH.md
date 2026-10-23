# Attune SQLx PostgreSQL cancellation patch

This directory vendors the published `sqlx-postgres` **0.8.6** crate with a narrow
local CancelRequest capability. It is not a new SQLx release, and the patch has
not been accepted upstream. Package version, features and dependencies remain
unchanged. It is paired with `../sqlx-core-0.8.6` for connected-peer capture.

## Upstream provenance

* Archive: <https://static.crates.io/crates/sqlx-postgres/sqlx-postgres-0.8.6.crate>
* Archive SHA-256: `db58fcd5a53cf07c184b154801ff91347e4c30d17a3562a635ff028ad5deda46`
* Upstream repository: <https://github.com/launchbadge/sqlx>
* Recorded upstream commit: `bab1b022bd56a64f9a08b46b36b97c5cff19d77e`
* Applied driver proposal SHA-256: `645a6c37acf7c635c623c3fbd237d785467da925c78ab100b355202d6a20905f`

`LICENSE-MIT`, `LICENSE-APACHE`, `.cargo_vcs_info.json`, `Cargo.toml` and
`Cargo.toml.orig` are preserved byte-for-byte from the archive. Both upstream
MIT and Apache-2.0 licenses remain available verbatim. Only the maintained Rust
files below and this provenance note differ from the published archive. The
maintained Rust files were formatted after applying the reviewed proposal.
The compatibility maintenance below is additional to that driver proposal.

## Maintained source and purpose

* `src/connection/cancel.rs`, added locally: provides opaque `PgCancelToken`,
  `PgConnection::cancel_token` and a consuming `cancel` operation. Encodes a
  startup-only CancelRequest, not a SQL statement or authenticated connection.
* `src/connection/mod.rs`: keeps cancellation options privately with the backend
  key and exports the capability type.
* `src/connection/establish.rs`: captures the original connection options at
  establishment, rather than asking application code to reconstruct routing.
* `src/connection/stream.rs`: captures the actual TCP peer before TLS, then
  reconnects cancellation to that complete `SocketAddr` using the paired core
  helper. IPv6 scope ID and flow information are not discarded. Original TLS
  options and certificate hostname remain in use; the peer IP is not substituted
  as TLS identity. Unix-domain cancellation retains the original socket path.
* `src/message/backend_key_data.rs`: redacts backend-key Debug output and rejects
  unsupported key-data lengths instead of truncating or indexing a short frame.
* `src/lib.rs`: re-exports `PgCancelToken` beside `PgConnection`.
* `src/connection/sasl.rs`: borrows the same digest bytes through `&u[..]` instead
  of deprecated GenericArray `as_slice`. SCRAM iteration, XOR and HMAC operations
  are unchanged; no cryptography dependency or logging is added.
* `src/listener.rs`: `next_buffered` uses receiver `try_recv` instead of deprecated
  `try_next`. A buffered value still becomes `Some(PgNotification)`; empty and
  disconnected states still become `None`. Listen registration, reconnection,
  notification consumers and asynchronous receive behavior are unchanged.
* `src/message/mod.rs`: retains the upstream `Flush` wire-format variant with
  an item-specific `allow(dead_code)` and a protocol-compatibility reason.
* `src/message/flush.rs`: retains the upstream `Flush` struct and encoder with
  an item-specific `allow(dead_code)` and a no-caller-in-this-build reason.

These warning treatments preserve the upstream protocol types and encoding
instead of deleting them. There is no module-wide or crate-wide warning
suppression, dependency change or compiler-version bump. The paired core patch
retains its existing atomic API for Docker Rust 1.98.1 compatibility rather than
requiring the Rust 1.99 replacement. No compiler-validation claim is made here.

## Capability and protocol limits

The new public capability has private PID/key/options/endpoint fields. It adds
no raw PID/secret constructor or getter, serialization, or caller-selected
alternate destination. Both Debug and Display are redacted. Keep the token
private to the owner of the original checked-out session. Do not send it after
that session has been returned to a pool.

SQLx 0.8.6 negotiates PostgreSQL protocol 3.0. Its BackendKeyData body is exactly
eight bytes: four for the PID and four for the cancellation key. Variable-length
protocol 3.2 keys are not supported by this patch. The cancel packet is the
16-byte startup packet with request code 80877102 and that captured PID/key.

One invocation opens one cancellation transport, negotiates the original TLS
policy, writes the request and waits for remote EOF. It sends no StartupMessage,
authentication exchange or SQL. The driver adds no detached task, retry or
deadline. The application owns the bounded channel permit, deadline and original
session; dropping the cancellation future drops its transport.

EOF means transport completion only. PostgreSQL sends no acknowledgement that
the key matched, a query stopped, a transaction rolled back or a session ended.
The application must observe the original operation settling before reuse.
Relays/proxies must implement PostgreSQL cancellation routing correctly; a clean
transport close does not certify their routing or cancellation support.

## Regression coverage to retain

* `src/connection/cancel.rs::tests::cancel_request_is_a_startup_packet_not_a_query_message`
* `src/connection/cancel.rs::tests::capability_formatting_is_fully_redacted`
* `src/message/backend_key_data.rs::rejects_unsupported_backend_key_lengths`
* Paired core test `src/net/socket/mod.rs::exact_tcp_target_preserves_scoped_ipv6_socket_addr`

These tests cover encoding, redaction, frame shape and exact-address retention.
Live deadline, TLS, proxy and target-settlement behavior needs separate owned
integration validation. This note does not claim those tests have passed.
