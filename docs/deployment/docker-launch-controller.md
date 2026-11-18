# External Docker launch controller (draft)

This is the one candidate backend for the remote CP launch contract. It is not
ready for a supported installation: actual CP file-publication/discovery effects,
all execution-only host consumers/producers, primary Compose integration and
packaged end-to-end validation remain incomplete. Existing published images and
the legacy primary Compose workers cannot satisfy this contract. Keep the app
and worker chart PRs draft until their combined checks and independent security
review pass. No release or production rollout is implied by these files.

Run `docker-compose.workload-host.yaml` as a separate project on the **workload
Engine host**. CP/API, PostgreSQL, RabbitMQ, master/signing keys and other
secret-bearing managers belong on a genuinely remote control-plane host. A
single-host owned test fixture supplies boundary evidence, not evidence of this
production placement. Execution-only action/sensor hosts use CP-issued scoped
enrollment, HTTPS/public CA and their own persistent encrypted custody; they
never receive the Engine socket or the controller session.

The trusted controller holds the Unix Engine socket and fixed local named cache
volume. It runs as root with only CHOWN, DAC_OVERRIDE and FOWNER, drops other
capabilities and uses `no-new-privileges` and a read-only root. Pack containers
remain uid/gid 1000, drop all capabilities and get private normal procfs, selected
RO input subpaths and their own writable stage/scratch. This project enables no
privileged container, DinD, host PID/network namespace or relaxed seccomp policy.
It does not claim network isolation or live disk/inode quotas.

The separate TLS proxy shares the controller's network namespace so it can reach
the literal-loopback backend. It shares no PID namespace, cache, session files or
Engine socket. It serves only TLS on port 8443 and forwards target/channel grants to
the backend. The controller independently calls the remote CP over verified
HTTPS. The proxy never receives its reusable CP bearer. Use an actual immutable
nginx image that supports the included config and `/usr/sbin/nginx` as uid 1000.
There is no invented proxy/agent/controller release digest in the project; the
local controller tag denotes source-build output only. Syntax/model tests are
not proof that an operator-selected proxy or application image implements the
contract.

Only port 8443 is published; the plaintext backend is fixed to 127.0.0.1 inside that
shared namespace. Host and caller headers pass through with exact body bytes;
hop-by-hop framing is handled by nginx/HTTP1.1. Request buffering is disabled,
stdin remains bounded to 8MiB, and access/runtime error logs are disabled to avoid
recording private grants or payloads. Both entrypoints clear inherited
environment before starting their fixed executable.

## Public configuration and private state

Build the controller image and pull the actual admitted runtime/probe images
**before** requesting its initial session. The CP-issued controller session lasts
at most 300 seconds; minting it before a cold image build is not a usable bootstrap.
Use the CP's actual persisted engine identity/daemon/platform registration and
operator-admitted immutable images. The generator below never registers an
engine, mints a credential, contacts Docker/CP or attests an image:

```bash
python3 scripts/generate-workload-controller-config.py \
  --api-origin "$REMOTE_CP_HTTPS_ORIGIN" \
  --engine-id "$CP_ALLOCATED_ENGINE_ID" \
  --cache-volume "$ATTUNE_LAUNCH_CACHE_VOLUME" \
  --workload-network "$ATTUNE_LAUNCH_NETWORK" \
  --probe-image "$ACTUAL_IMMUTABLE_PROBE_IMAGE" \
  --allow-image "$ACTUAL_IMMUTABLE_RUNTIME_IMAGE" \
  --output ./controller-config.json
```

The CP image/profile admission and runtime proof must match this Engine's actual
daemon and platform; replacing either requires a new engine identity. The
controller verifies these bindings, selected image platforms and an owned random
sentinel through the exact same named-volume/subpath before accepting work.
Caller requests can provide only admitted launch IDs/digests, never this config,
images, commands, host paths, Docker HostConfig or arbitrary resource selectors.

Install the generated public JSON and the separately CP-issued `WorkloadSession`
JSON as **root-owned regular single-link files**, without symlink aliases. Config
must not be group/other writable; the initial session must be `0600` or `0400`. File
secret declarations in Compose do not fix host file ownership/mode. Startup
checks the actual files and fails closed. The supplied TLS key must be readable
only by the proxy uid 1000 (for example `0400`); its public chain must match the real
CP-registered HTTPS controller origin. The CP trust bundle is public and mounted
read-only; `ATTUNE_LAUNCH_CP_CA_BUNDLE` can select a site or owned-test CA bundle
without disabling certificate/hostname verification or changing host trust.

Set these operator values before rendering/building the external project:

- `ATTUNE_LAUNCH_CONFIG_FILE`, `ATTUNE_LAUNCH_SESSION_FILE`: the installed files.
- `ATTUNE_LAUNCH_CACHE_VOLUME`: exact same fixed local volume as the JSON config.
- `ATTUNE_LAUNCH_STATE_VOLUME`: a separate retained private session-state volume.
- `ATTUNE_LAUNCH_NETWORK`: the fixed workload bridge from the JSON config.
- `ATTUNE_LAUNCH_PROXY_IMAGE`: the actual validated immutable proxy image.
- `ATTUNE_LAUNCH_TLS_BIND`, `ATTUNE_LAUNCH_TLS_CERTIFICATE`,
  `ATTUNE_LAUNCH_TLS_PRIVATE_KEY`: owned HTTPS listener and its real chain/key.
- Optionally `ATTUNE_LAUNCH_DOCKER_SOCKET` and the public CP CA bundle above.

The reusable session lives under the controller's exclusive `0700` state root,
separate from the launch cache, proxy and execution-host custody. Its `0600` file is
fsynced and atomically published before the in-memory credential is replaced.
Retained state wins over an initial file. Malformed/expired retained state and
uncertain persistence stop admission and require explicit CP bootstrap; nothing
silently resets credentials or falls back to an initial/unlimited/master token.
The initial bootstrap/long-outage operational path still needs full integration
with W2's CP issuer. Do not delete volumes or keys to hide a recovery failure.

## Validation and remaining integration

`python3 scripts/test-workload-controller-compose.py` checks public config and the
Compose model without starting Docker or contacting a service. It verifies that
authority remains controller-only and that missing inputs fail rendering. Its
synthetic pins/files are not installable images, sessions or TLS evidence.

`scripts/test-workload-controller-project.py --owned-engine-socket <socket>` is
the real image/project lane. Before a cold build it tests the pinned Nginx image
as uid1000 with no capabilities, read-only root and bounded private tmp: actual
bound-key readability, `nginx -t` and foreground master startup. Every compiled
module's temporary path stays under that tmp. The probes acknowledge owned
Stop+Wait+Remove; early sanitized category/status records are retained under
`.scratch/e2e-diagnostics/` without raw Nginx logs or private key bytes.
It then builds the candidate controller image, uses owned
TLS keys/public CA and a deliberately denying CP protocol fixture, then checks
actual startup/binding, HTTPS trust/hostname failures, proxy grants, external
plaintext denial, authority mounts/capabilities and private-marker log absence.
It never fabricates successful launch/stdin/publication authorization. Its
protocol fixture does not prove the actual CP ledger/effects or host consumers.
It requires an explicitly owned disposable Engine and scoped ownership changes
to its own generated files; missing prerequisites fail. No host security policy,
production service or unrelated volume may be changed. Unknown startup outcomes
retain owned volumes rather than treating a timeout/empty lookup as absence.

Actual tests must use source-built controller/agent/helper images, owned CP/DB
fixtures and ordinary owned Engine containers. Required evidence includes
HTTPS/origin/input binding, per-stage launch and cancellation/descendant removal,
exact ZIP publication and reuse, all runtime/sensor/candidate consumers, raw-log
custody, file-artifact inventory/publication/restart/lost-ACK and the actual
default fresh-install entrypoints. Missing prerequisites fail, not skip. A
successful config render, static health response or capability marker does not
establish that evidence. See [the boundary status](workload-isolation.md).
