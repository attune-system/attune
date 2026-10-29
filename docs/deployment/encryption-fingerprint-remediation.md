# Encryption fingerprint exposure and remediation

## Exposure

Previously, `hash_encryption_key(configured_secret)` returned hexadecimal
SHA-256 of the configured secret. `crypto::derive_key` uses those same 32 bytes
as the shared AES-256-GCM key. A database reader could decrypt ciphertext without
the configured secret or application decrypt permission. Authenticated manifests
using that AES key could also be forged. This affects every encrypted class using
the same configured secret, not just Keys.

Never print old fingerprints. Treat replicas, dumps, WAL, backups, exports,
diagnostics and retained evidence containing them as encryption-key material.

## Implemented cryptographic contracts

Key records now use `crypto::keystore`, with HKDF-SHA-256 implemented by `ring`:

```text
salt = b"attune:keystore:hkdf-sha256\0"
info = b"attune:keystore:aes-256-gcm\0" || i64_positive_id_big_endian
record_aes_key = HKDF-SHA256(salt, configured_secret_utf8).expand(info, 32)
aad = b"attune:keystore:value\0" || i64_positive_id_big_endian
fingerprint = hex(SHA256(b"attune:keystore:key-fingerprint\0" || record_aes_key))
```

Every prefix includes its final zero byte. IDs must be positive and use exactly
eight big-endian bytes. Callers supply the trusted loaded row ID for derivation
and authenticated additional data. The repository reserves the sequence ID
before encrypting and inserting, within the API's existing creation transaction.
No placeholder or plaintext row is inserted. Rollbacks can leave sequence gaps.

Names are mutable and repeat across owner scopes. Renaming `key.name` changes
neither the key nor its authenticated context. Moving ciphertext to a different
record fails authentication. Normal updates cannot change the ID. Preserve IDs
on restore/import, or explicitly re-encrypt under the replacement IDs. Exposure
of one record AES key does not disclose the other record keys or master secret;
master-secret disclosure still compromises all records.

Generic recorded fields, snapshots, pack-source headers, inquiry payloads, OIDC
state, runtime logs, file bodies and completion manifests retain their existing
shared AES derivation, `SHA256(configured_secret_utf8)`. They are separate from
the Key-record HKDF domain and are not assigned invented Key identities. Their
new shared verifier is lowercase hexadecimal
`SHA256(b"attune:encryption-key-fingerprint\0" || shared_aes_key)`. It is not usable
AES material. Per-entity isolation for those classes is separate work.

The encoding remains one Base64 string containing the 12-byte random nonce,
ciphertext and 16-byte GCM authentication tag. There is no colon separator or
new version envelope. Key derivation/AAD changed, so historical Key ciphertext
requires explicit conversion. There is no runtime legacy fallback or dual reader.
Generic ciphertext derivation is unchanged, but old generic verifier checks fail
closed until metadata conversion.

Use a cryptographically random configured secret with at least 256 bits of
entropy, for example 32 random bytes encoded as hex. A 32-character password
is not equivalent. The input-length check and public HKDF salt do not harden
weak passwords; fingerprints permit offline guessing.

## Storage and reader inventory

Direct fingerprint columns exist on `key`, `execution_secret_value`,
`runtime_accepted_input`, `runtime_input_receipt`, and
`runtime_native_input_snapshot`.

- Key create/update/read/signing, dashboard/queue selectors, explicit Key
  resolution, declared input materialization and inherited input Key reads use
  ID-bound Key encryption. Key API DTOs do not return fingerprints. Hiding
  metadata alone would not fix database disclosure.
- `secret_values::prepare_secret_values` creates shared verifiers, checked by
  `restore_secret_values` and API disclosure. Recorded fields cover component
  configuration, events, queues, enforcements, execution config/results, workflow
  variables, inquiries and sensor inputs. Worker restoration and sensor-schema
  delivery use these shared helpers.
- Accepted inputs retain field fingerprints inside `body.fields`. Native
  snapshots retain them inside typed source bodies. Receipt proofs, inherited
  inputs, native wait/iteration/transition/cache materialization and success
  metadata share these contracts. Copies also exist in event-origin snapshots,
  `execution_secret_value`, and retained file-evidence JSON. Body digests,
  dependent references, immutable guards and byte budgets protect these records.

New helper calls do not sanitize old fingerprints copied from existing rows.
A mixed or unconverted installation is not remediated.

## Offline upgrade procedure

`attune-encryption-convert` is an explicit offline operator tool backed by
`repositories::encryption_conversion`. It never loads deployed-service profiles
or server configuration. It requires dedicated environment variables:

```text
ATTUNE_ENCRYPTION_CONVERSION_DATABASE_URL
ATTUNE_ENCRYPTION_CONVERSION_SECRET
```

Supply these securely through the operator's environment, not command arguments
or shell history. Build/run the tool from the reviewed source:

```bash
cargo build --locked -p attune-common --bin attune-encryption-convert
target/debug/attune-encryption-convert --help
target/debug/attune-encryption-convert --dry-run
# After stopping ALL service writers and reviewing the dry-run:
target/debug/attune-encryption-convert --apply --offline-writers-stopped
```

1. Back up the database and matching file/object storage under secret-grade
   access controls. Retain the old binaries and configured master secret for
   recovery. Rehearse on a disposable restored copy first.
2. Stop API, executor, worker/agent, sensor, notifier and supervisor services,
   scheduled jobs and external writers. Settle outstanding file PUT operations.
   Fence reconnects until conversion and verification finish. The acknowledgement
   flag is not proof that distributed writers have stopped.
3. Apply the new forward migration through the normal migration runner. A DBA
   must create `attune_encryption_maintenance NOLOGIN` if absent, then grant it
   directly to a dedicated offline login, never a runtime login. That login also
   needs SELECT on converted tables and existing retained-evidence dependencies.
   Keep the application schema on its trusted `search_path`. Do not use a runtime
   identity that owns the schema or can alter guards.
4. Set the two environment variables above to the explicit target and its
   unchanged master secret. Run `--dry-run`, review the count-only report, then
   `--apply --offline-writers-stopped`. Both authenticate affected values and
   validate retained evidence. Unknown verifiers, corruption, lock timeout or
   failed integrity checks abort; apply publishes one atomic transaction.
5. Run `--dry-run` again. All converted counts must be zero. Restart only the
   new binaries with the same master secret, then verify Key read/update/signing,
   a worker/sensor restoration, and authorized reads of retained artifacts.
   Revoke the temporary role membership after verification.
6. On conversion failure, keep writers stopped, correct the prerequisite and
   retry. Interruption rolls back the transaction; rerunning authenticates current
   rows without rewriting them. After a successful conversion, binary downgrade
   alone is unsafe. Recover by restoring matching old data, storage and binaries
   together, without discarding post-restart writes silently.

The converter re-encrypts only historical Key values under their immutable IDs.
It updates known fingerprint fields in recorded values, receipts, accepted inputs,
all typed native snapshot sources and event snapshots, including nested originals,
dependent digests and normalized copies. User JSON, identities, provenance,
permissions, schema bindings, pins and retained-byte accounting stay unchanged.
Unchanged generic ciphertext, file/object versions, log bodies and authenticated
manifests are not rewritten. Their envelopes do not contain affected fingerprints.

Exclusive graph admission and table locks protect the transaction. A separate
operator capability permits only exact row transitions while normal immutable
guards stay active. Private audit rows contain identities, digests and counts,
not old verifier bytes. Dry-run rolls back without writing. Diagnostics are
redacted. Metadata planning is bounded to 256 MiB; larger installations are not
supported by this bounded tool and must not be upgraded without an approved
conversion approach. No history is deleted and no applied migration is rewritten.

## Separate security decision

Old stored verifier bytes were usable AES material. Correcting metadata does not
undo historical disclosure or revoke copied AES material, especially for generic
ciphertext that keeps the shared derivation. Actual exploitation is not established.
Incident response, master-key rotation, external-credential rotation and backup
handling remain separate operator security decisions. Changing `ENCRYPTION_KEY`
alone strands historical ciphertext; this tool is not a rotation framework.
