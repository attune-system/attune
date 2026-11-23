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

Enable this profile through the matching first-party chart. Use a release
that includes the restricted manager migration and the API notification
payload changes on all replicas. The chart creates dedicated StatefulSets
for configured action and sensor pools. An action pool keeps its encrypted
log spool in a per-slot RWO claim. Both roles use writable local pack,
runtime, and artifact directories and keep execution and artifact transfer on
the Attune API.

The post-migration Job installs the installation-specific groups, the database
functions, and a dedicated supervisor login. The Job's database administrator
credential is scoped to its Pod. The supervisor uses its own TLS-verified
connection with permission to execute the provisioning functions.

Each manager presents its regular Attune session bearer and a projected
Pod-bound token. The API asks Kubernetes to verify the expected audience and
ServiceAccount, checks the live Pod UID and owning StatefulSet, then records a
request for that exact enrolled instance. Restrict Pod and ServiceAccount
management in the manager namespace to trusted deployment operators. The
manager generates the SCRAM password in memory and sends a verifier through
the scoped API. The supervisor installs the verifier and drops it from the
request row. A manager establishes and checks its scoped connection over
verified TLS, then keeps the prior connection until the next generation is
usable. Generation changes preserve the per-process login and never carry
forward to a replacement process.

The supervisor checks bound Pod identities in bounded batches. A confirmed
missing or replaced Pod, withdrawal, or credential expiry retires the login.
An unavailable Kubernetes API leaves the credential on its existing lease.
The login fence commits independently of manager-held binding locks.
Subsequent passes terminate existing connections, wait out in-flight
authentication, and remove the disabled role. The binding and credential
metadata retain the retirement record.

Provide a verified PostgreSQL TLS endpoint and a dedicated pool Secret per
role. Put manager credentials outside workload paths and egress only to the
Attune API, the PostgreSQL endpoint, and the destinations required by the
workloads. The chart's inventory, labels, and taints are operator-controlled.
Use the matching chart release documentation for the Job and Secret settings.

Test both an action pool and a sensor pool first. To roll back, scale the
restricted pools to zero while the supervisor task remains enabled. Observe
retirement, then remove the configuration and provisioning grants. An
administrator can retire managed logins through the guarded SQL operations
during a supervisor outage.

## Verification

Run the guarded-role regression with an owned disposable cluster on each
supported PostgreSQL baseline.

```sh
bash scripts/verify-manager-db-roles.sh postgres:16-alpine
bash scripts/verify-manager-db-roles.sh postgres:18-alpine
```
