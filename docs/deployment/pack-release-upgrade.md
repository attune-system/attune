# Pack release upgrade

Attune 0.6 freezes installed packs into immutable, digest-verified releases. On
the first API startup after upgrading, Attune reads each legacy pack's
`storage_path`, creates a deterministic archive and manifest, uploads the exact
archive to configured blob storage, and activates the release in one database
transaction.

The API does not create placeholder release rows. Metadata-only packs without a
`storage_path` need no release. If a stored path is not a directory or cannot
produce a valid archive, the API logs the affected pack refs and keeps the
platform API ready for repair. Existing pack rows and files remain in place.

## Mandatory repair procedure

For every failed pack named in the API startup logs:

1. Restore the exact installed pack directory, including `pack.yaml`, under a
   path visible to the API container.
2. Force-register that directory:

   ```bash
   attune pack register <server-visible-pack-directory> --force --skip-tests
   ```

3. Repeat the pack inspection and repair request. `/health/ready` covers the
   database and exact platform catalog only; it does not close for pack content.

`/health/content` is a transitional deployment check for active core and fresh
action/sensor hosts. It returns HTTP 503 while those are absent. It does not yet
prove required-pack locks or candidate evidence; issue #75 replaces this coarse
contract. `/health/live` checks only the API process.

If the original bytes cannot be recovered, install a known pack version with
`attune pack install <source> --force --skip-tests`. This is a replacement, not
a reconstruction of the missing legacy release.

Deleting a pack removes its release records and schedules release objects for
cleanup. Historical execution, enforcement, queue-item, and sensor-workload
rows retain their release digest and executable snapshot. Their database
release ID becomes `NULL` because the release record no longer exists.
