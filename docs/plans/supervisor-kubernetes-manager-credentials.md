# Optional Kubernetes manager credential provisioning

Ship the restricted manager PostgreSQL migration, role setup Job, supervisor
reconciler, and manager connection support together as one opt-in feature.
PR #110 has a verified SQL prototype. Its guarded session operations provide
development evidence, while the first supported rollout needs automated
issuance, rotation, retirement, and a running manager that uses the login.

Keep the shared notification payload change in PR #110 with the restricted
manager feature. Apply the notification migration and roll out the matching
API publishers everywhere before enabling manager database access.

An enabled `attune-supervisor` runs the credential reconciler alongside its
maintenance tasks. The existing Attune API remains the manager's entry point.
The first release uses the restricted SQL session operations to prove the
lifecycle. Scheduling, action and sensor workloads, and artifact transfer
continue through their current paths.

## Implementation

The application provides the opt-in task, Kubernetes proof, SCRAM rotation,
and the scoped connection for both manager roles. The first-party chart
wires provisioning, dedicated ServiceAccounts, projected tokens, private
mounts, and the per-slot action spool. Keep the release gate below for a
canary using the matching application and chart builds.

## 1. Prove the database provisioning authority

- Extend the post-migration Job to install the installation-specific action and
  sensor groups and narrowly granted SQL operations for issuing, rotating, and
  retiring managed logins. Move the checked binding logic from `bind.sql` into
  the shared database contract so manual and automated provisioning enforce
  the same checks. Keep privileged role ownership with the Job and dedicated
  PostgreSQL roles. The supervisor uses a separate connection identity limited
  to the provisioning operations, with no direct `CREATEROLE`, broad table
  access, or `pg_signal_backend` grant.
- Require a trusted current session and an exact role, pool, slot, and process
  match in the provisioning transaction. Generate login names from the
  installation and persisted session. Keep a durable record of each login's
  identity and retirement. Use the database clock, execute role creation and
  binding atomically, and give login passwords a bounded validity period.
- Prove that a non-superuser installation can grant the two permitted groups,
  disable managed logins, and close only their database sessions. Check the
  site's PostgreSQL authentication and managed-provider permissions before
  documenting that deployment as supported. Exercise PostgreSQL 16 and 18.

## 2. Bind Kubernetes identity to the Attune process

- Use the existing manager session bearer plus a projected Pod-bound
  ServiceAccount token with an Attune-specific audience for a private manager
  credential request. Have the Attune API verify the token via TokenReview
  and check the live Pod UID, ServiceAccount, namespace, and admitted
  workload. Derive the permitted role, pool, and slot from chart-controlled
  workload configuration. Require a fresh proof for each issuance or rotation.
  A manager's reported Pod name or shared pool secret alone conveys no
  Kubernetes identity.
- Persist the verified Pod UID against the exact enrolled process UUID
  and session ID. A process restart within the same Pod still requires a new
  Attune enrollment and database login. Recheck live session and binding
  state inside the provisioning transaction. Keep the provisioning module
  callable through typed repository methods.
- Start with SCRAM over verified TLS. Prototype manager-generated passwords
  and SCRAM verifiers. The manager retains its password in memory and sends
  only the verifier through the scoped HTTPS request. Store the verifier
  transiently in the private request record and remove it on completion,
  expiry, or withdrawal. Keep it out of all logs, audit details, and
  notifications. Retry with the same request ID and verifier after a lost
  response. A replacement process generates its own password. Confirm
  compatible verifier generation and TLS authentication before choosing the
  production Rust dependency.

## 3. Run reconciliation and connect the managers

- Add an off-by-default setting and a separately supervised async task with
  bounded batches, retries, and graceful shutdown. Share the supervisor
  binary and Deployment. Use transactional database claims to serialize
  per-login changes across replicas. Leave the retention lock and maintenance
  cycle available during Kubernetes requests and provisioning retries.
- Issue pending attempts and report readiness via the existing API. A manager
  opens its restricted connection only after the matching request is ready.
  Rotate credentials for the same live process with a monotonically fenced
  request and swap its connection pool after the new login succeeds. Keep
  connection counts bounded. Treat PostgreSQL unavailability as a health
  signal for this optional path. Preserve the API-backed manager lifecycle.
- Reconcile withdrawal, session expiry, verified Pod deletion, disabled pools,
  and superseded processes. Commit the login fence before closing old
  connections. Fence the login using credential state independent of
  manager-held binding row locks. Retry session cleanup without issuing
  another login for a retired
  instance. During Kubernetes API outages, pause new issuance requiring fresh
  proof and continue expiry cleanup from authoritative database state. Retry
  transient Pod lookup failures before acting on the Pod state.

## 4. Validate and roll out

- Use the first-party chart to wire the feature flag, post-migration Job,
  provisioning database credential, TLS trust, dedicated manager
  ServiceAccounts, and projected tokens. Grant the API only the TokenReview
  verb and required Pod reads in the allowed namespaces. Grant the supervisor
  only the Pod reads required for reconciliation. Managers need no Kubernetes
  API permissions, Secret-write access, or role administration. Keep both
  manager credentials out of child environments and mounts.
- Use owned disposable PostgreSQL and Kubernetes fixtures. Prove that
  action and sensor managers can inspect their own sessions through their
  restricted database logins while running ordinary workloads via the API.
  Cover mismatched identities, revoked grants, forbidden SQL operations,
  duplicate requests, lost responses, Pod replacement and same-Pod restarts,
  rotation, revocation of already-open connections, supervisor failover,
  and Kubernetes and database outages. Verify that disabled deployments
  require no Kubernetes configuration or provisioning credential. Measure
  enrollment-to-credential latency, rotation time, and database connection
  count at a representative pool size.
- Gate the restricted-profile release on passing the full login lifecycle
  with both manager roles. Within that release, apply the private schema
  migration before starting the binaries that depend on it. Run the role
  setup Job only where the opt-in feature is configured. Then enable one
  action pool and one sensor pool.
- Monitor issuance failures, request age, revocation lag, and stale sessions.
  For rollback, stop issuing, move canaries to API-only mode, and reconcile
  all managed logins to retirement before removing the provisioning
  permissions. Document an administrative cleanup procedure for a supervisor
  outage.

See [restricted manager PostgreSQL roles](../deployment/restricted-manager-postgresql.md),
[manager enrollment](../deployment/api-only-managers.md#enrollment-contract),
[Kubernetes Pod-bound token verification](https://kubernetes.io/docs/reference/access-authn-authz/service-accounts-admin/#verifying-and-inspecting-private-claims),
and [PostgreSQL role attributes](https://www.postgresql.org/docs/16/sql-createrole.html).
