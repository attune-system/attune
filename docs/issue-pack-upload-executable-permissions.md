# Pack upload strips executable permissions, preventing native sensors from starting

## Summary

In Attune **0.8.0**, uploading a pack through the API loses executable permissions during archive extraction.

Files that are executable in the source pack become non-executable in the published release. Workers then correctly reproduce those non-executable permissions when downloading the release.

This prevents native sensors from starting. We reproduced the issue with **core pack 1.0.3**, where `core.timer_sensor` fails before creating its stdout/stderr artifacts.

## Environment

- Attune application: `0.8.0`
- Helm chart: `0.9.0`
- Core pack: `1.0.3`
- Kubernetes deployment with API-based pack transport and GCS object storage
- Pack installed using `attune pack upload <path> --skip-tests`

The core pack files came from the official `ghcr.io/attune-system/attune/init-packs:core-1.0.3` image.

## Steps to reproduce

1. Obtain core pack 1.0.3 from the official bootstrap image, retaining file permissions.
2. Confirm the timer launcher and native binaries are executable.
3. Upload the pack through the API.
4. Create an enabled rule using:

   ```yaml
   trigger_ref: core.intervaltimer
   trigger_params:
     interval: 1
     unit: seconds
   action_ref: core.echo
   action_params:
     message: timer smoke test
   enabled: true
   ```

5. Inspect the sensor worker's service logs and downloaded release files.

## Expected behavior

The upload and release-publication process preserves **executable intent**. The sensor worker can launch `core.timer_sensor`, which emits events and produces rule enforcements and executions.

## Actual behavior

The sensor manager discovers the active rule, resolves `core.native`, and provisions the sensor token, but cannot spawn the process:

```text
Sensor process binary is not executable

Failed to start sensor process for 'core.timer_sensor'
with runtime 'core.native' (native=true):
Permission denied (os error 13)
```

Observed permissions:

| File | Source pack | Worker release cache |
|---|---|---|
| `sensors/timer-sensor-launcher` | `0755` | `0644` |
| `sensors/bin/linux-amd64/attune-core-timer-sensor` | `0755` | `0644` |
| `sensors/bin/linux-arm64/attune-core-timer-sensor` | `0755` | `0644` |

SHA-256 checks confirmed that all three cached files have the same contents as the official image. The defect is permission metadata loss, not missing or corrupted binaries.

No sensor stderr is available because the process never starts. No timer events are emitted, so no enforcements or executions follow.

`core.echo` can still execute because its script is invoked through Bash; that smoke test does not validate direct native execution.

## Root cause

The affected path is:

```text
CLI upload archive
  → API archive extraction
  → release staging and manifest generation
  → GCS release archive
  → worker extraction
```

The following source paths refer to the Attune application repository at release 0.8.0, not this deployment repository:

- `crates/cli/src/commands/pack.rs`
  - CLI tar creation uses `append_path_with_name()`, retaining source file modes.
- `crates/api/src/routes/packs.rs`
  - `upload_pack()` calls `safe_unpack()`, which delegates to the shared archive extractor.
- `crates/common/src/pack_registry/archive.rs`
  - Extraction writes contents into newly created files but does not restore executable intent from the tar header.
  - The `write_bounded()` helper creates ordinary files without restoring permissions.
- `crates/common/src/pack_registry/storage.rs`
  - Release generation derives each manifest entry's executable flag from the extracted file's permissions.
  - By this point, executable intent has already been lost.
- `crates/common/src/pack_transport/api.rs`
  - Worker extraction already restores executable intent from release archive headers.
  - However, those headers now describe the files as non-executable.

GCS stores the generated archive bytes; it is not the permission-loss point.

## Proposed durable fix

1. **Preserve sanitized executable intent during API archive extraction.**
   - Read the tar entry's execute bits before writing the file.
   - After successful bounded extraction, normalize regular files to:
     - `0755` when executable intent is present.
     - `0644` otherwise.
   - Follow the existing worker-extraction approach where practical.

2. **Retain archive security protections.**
   - Do not preserve arbitrary ownership or privileged permission bits.
   - Do not enable unrestricted permission preservation.
   - Keep traversal, symlink/hardlink, duplicate-entry, entry-count, and size protections intact.
   - Keep directory permissions suitable for traversal.

3. **Review other archive-ingestion paths using the shared extractor** for equivalent permission loss.

4. **Publish corrected pack releases after fixing ingestion.**
   - Existing releases already encode incorrect executable metadata.
   - Use a new core pack version and digest rather than modifying an immutable release in place.
   - Update bootstrap image/chart pins accordingly.

## Required regression coverage

- API extraction preserves executable intent for both scripts and native binaries.
- Non-executable files remain non-executable.
- Privileged permission bits are not retained.
- Existing archive-security tests continue to pass.
- Release manifests and generated tar headers retain executable intent.
- Worker downloads preserve that intent.
- End-to-end test: upload a native-sensor pack, enable a rule, and verify:
  - Sensor process starts.
  - An event is stored.
  - An enforcement is created.
  - The resulting execution completes.

## Acceptance criteria

A clean installation using the corrected application and pack release runs the core timer sensor through API/GCS pack transport **without manual `chmod`, cache modifications, or worker-specific startup workarounds**.

## Workaround limitations

Changing permissions in a worker's local cache can temporarily unblock development, but does not repair the stored release. A fresh cache or replacement pod will reproduce the failure. Re-uploading through unchanged Attune 0.8.0 also repeats the permission loss.
