# Restricted PostgreSQL roles for managers

The restricted role foundation binds individual PostgreSQL logins to the existing
action and sensor manager sessions. It adds guarded session inspection and
self-withdrawal. Manager binaries still enroll and run through the Attune API,
including placement, execution, sensor startup, and data transfer.

## Role and session setup

The migrations install the private binding table and functions. A DBA explicitly
enables the functions for selected database roles. Use one dedicated
`NOLOGIN` role for operation ownership and two separate `NOLOGIN` capability
groups for actions and sensors. The operation owner must have no members.
Keep schema ownership, schema creation, and platform administration with the
trusted database installation roles.

For a disposable installation, create these roles as the DBA.

```sql
CREATE ROLE manager_sql_owner NOLOGIN;
CREATE ROLE manager_action_sql NOLOGIN;
CREATE ROLE manager_sensor_sql NOLOGIN;
CREATE ROLE worker_boot_1 LOGIN;
GRANT manager_action_sql TO worker_boot_1 WITH INHERIT TRUE, SET FALSE, ADMIN FALSE;
```

In an installation that disables the database’s default connection grants, grant
`CONNECT` to the intended logins. For a networked fixture, use verified TLS and
the site's authenticated PostgreSQL connection configuration. Keep login
credentials outside child processes and their mounts.

Install the role grants in the migration schema.

```sh
psql -v app_schema=attune -v operation_owner=manager_sql_owner \
  -v action_group=manager_action_sql -v sensor_group=manager_sensor_sql \
  -f deployment/worker-db-roles/provision.sql
```

Bind the login to a verified live API enrollment. Supply the role, pool, slot,
and exact process UUID from that enrollment. Create a fresh login for each
replacement boot.

```sh
psql -v app_schema=attune -v login_name=worker_boot_1 \
  -v manager_role=action -v manager_pool=build -v manager_slot=worker-1 \
  -v manager_instance="$instance" -f deployment/worker-db-roles/bind.sql
```

The action and sensor entry points are respectively `manager_db_action_session`
and `manager_db_sensor_session`. Each accepts the enrolled instance UUID and an
optional withdrawal flag. An authorized call returns the session ID, expiry
and closure time, if closed. The authenticated login must match the stored role
OID and login name. The binding and session stay locked until the calling
transaction ends. An unbound, revoked, stale or mismatched request returns no row.
Repeated withdrawal returns the same closure time within the original session
lifetime. Revoke a login's database authority by setting its binding's
`revoked_at` as the DBA. Retain its binding to prevent accidental reuse. API
enrollment-grant changes and database binding revocation are separate controls.

## Deployment boundary

The restricted profile shares operational notifications on the existing
PostgreSQL database. Managers can observe cross-pool identifiers, routing
references, status, and the timing of activity. Treat notifications as wakeup
hints and verify the underlying record through its authorized operation.

Rule parameters, artifact names and progress messages, event source
references, queue item keys, and trace tags are available through the
corresponding authorized APIs. The PostgreSQL payloads retain status,
identifiers, and numeric progress. Use API-only operation for pools that
require cross-pool activity privacy.

Apply the shared-notification migration and update all API replicas before
granting direct database connections to managers. The API publishes rule
lifecycle notifications independently of the database trigger functions.

Keep the platform signing and encryption keys, broker rights, and object-store
rights in the control plane. The existing API remains responsible for effects
that need those privileges or a trusted commit boundary.

## Kubernetes provisioning

Use a post-migration Job for shared database setup and a control-plane
reconciler for process credentials. Enable manager database access only when
the runtime supports the per-instance binding flow. The SQL in this change
supports DBA provisioning and verification. Manager processes still use the
Attune API for work.

1. Run the Job after migrations succeed and the API rollout uses the shared
   notification contract. Ensure the operation owner and separate action and
   sensor groups exist as dedicated `NOLOGIN` roles. Use installation-specific
   role names because PostgreSQL roles are cluster-wide. Run
   `deployment/worker-db-roles/provision.sql` with the schema and those role
   names. Make retries check existing roles and grants before continuing. Keep
   the Job's database administration credential scoped to the Job.
   [Kubernetes Jobs](https://kubernetes.io/docs/concepts/workloads/controllers/job/)
   · [PostgreSQL roles](https://www.postgresql.org/docs/16/sql-createrole.html)
2. Reconcile one restricted `LOGIN` per trusted manager incarnation. Derive
   the pool and stable slot from the controller-owned workload. Verify the
   manager's live API enrollment and its exact process UUID before applying
   `deployment/worker-db-roles/bind.sql`. Track the Pod UID together with
   that process UUID because a container can restart within the same Pod.
   Grant the appropriate action or sensor group with inheritance enabled
   and role switching and administration disabled.
   [Kubernetes Pod lifecycle](https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/)
   · [PostgreSQL membership grants](https://www.postgresql.org/docs/16/sql-grant.html)
3. Deliver the login as a read-only credential file scoped to the manager
   container, using a short-lived SCRAM secret over verified TLS or a
   PostgreSQL client certificate mapped to the login. Keep the file outside
   workload mounts. Give Kubernetes Secret access to the provisioning
   identity and use encryption at rest if credentials are stored as Secrets.
   Limit network egress to the API and the database endpoint needed by the
   restricted profile.
   [Kubernetes Secret security](https://kubernetes.io/docs/concepts/security/secrets-good-practices/)
   · [PostgreSQL client certificates](https://www.postgresql.org/docs/16/auth-cert.html)
4. On replacement or withdrawal, revoke the old database binding and
   prevent new connections with its login. Close any remaining sessions and
   retire the credential when safe. Keep the retired binding as evidence
   against reuse. Provision a new login and binding for the replacement.
   [PostgreSQL role changes](https://www.postgresql.org/docs/16/sql-alterrole.html)

The reconciler needs an authenticated way to obtain the enrolled instance
and hand the resulting credential to that same process. That handoff and
manager connection support are chart and application integration work.

## Verification

Run the guarded-role regression with an owned disposable cluster on each
supported PostgreSQL baseline.

```sh
bash scripts/verify-manager-db-roles.sh postgres:16-alpine
bash scripts/verify-manager-db-roles.sh postgres:18-alpine
```
