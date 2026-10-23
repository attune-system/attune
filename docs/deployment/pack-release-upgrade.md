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

## Repair releases with lost executable permissions

Older archive ingestion in Attune 0.8.0 discarded executable permissions from
uploaded TAR files and installed TAR or ZIP files. Affected releases record
native binaries and launchers as non-executable even when their contents are
correct. Worker downloads reproduce that metadata; object storage does not
cause the permission loss.

With the ingestion fix, Unix extraction retains only executable intent. Regular
files become `0755` if any execute bit is set in the archive, otherwise `0644`.
ZIP entries without Unix permission metadata are non-executable. Archive
ownership, privileged bits, and directory modes are not restored. Published
release trees remain read-only, with executable intent retained in their
manifests and generated archives.

Existing immutable releases are not repaired by upgrading the application or
changing a worker cache. After deploying the fixed API:

1. Obtain a pack source with executable launchers and native binaries.
2. Publish and install a new pack version through the fixed ingestion path.
   Do not overwrite the affected version or digest.
3. Update bootstrap image and chart pins when the corrected release is available.
4. Verify the timer sensor produces an event, an enforcement, and a completed
   execution on a worker with a fresh release cache. Running `core.echo` alone
   does not test native execution permissions.
