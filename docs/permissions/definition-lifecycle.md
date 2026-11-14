# Definition lifecycle authorization

Definition controls authenticate before effects and require explicit IAM on the
actual target. API-created definitions are ad hoc; a pack association or ref
prefix does not establish managed pack ownership.

| Controls | Resource / action | Credentials |
| --- | --- | --- |
| Workflow create, including file creation | Actions / Create on proposed ref and actual pack | Access |
| Workflow delete | Actions / Delete on companion action, or workflow ref if absent | Access |
| Trigger and sensor create/delete | Triggers / Create or Delete | Access |
| Trigger, sensor and rule enable/disable | Triggers or Rules / Update | Access |
| Rule enabled-state transition through PUT | Rules / Update on locked current target | Access |
| Pack workflow sync | Packs / Install on actual pack | Access |
| Pack workflow validation | Packs / Read on actual pack | Access |
| Server-path dependency inspection | Packs / global Read | Access |
| Server-path environment build | Packs / global Install | Access |

Server-path endpoints have no trusted database target before opening the supplied
path, so a caller-provided pack identity cannot satisfy a scoped grant. Their
authorization runs before filesystem parsing or subprocess execution. Filesystem
confinement remains a separate requirement.

The new lifecycle controls reject delegated Execution and service credentials,
even if the attributable identity has human grants. Recorded execution authority
remains disabled and missing admissions fail closed. Existing workflow and trigger
update routes retain their current IAM credential contracts; sensor metadata
update remains Access-only. Rule PUT preserves its existing Access/Execution IAM
and token ceilings for metadata edits and unchanged full-form enabled values.
Only an actual enabled-state transition, compared against the locked current row,
requires the same fresh Access Rules/Update check as the dedicated toggle routes.
Mixed enabled/behavior updates must also satisfy the independent behavior,
delegation and input-authority requirements; toggling alone neither requires
Actions/Execute nor recaptures runtime authority. Enabling retains live placement
admission. Dedicated service operations retain their own typed credential contracts.

Authorization precedes row changes, cache tombstoning, file writes and lifecycle
notifications. Enabling still runs placement admission; sensor configuration
binding and workflow companion ownership checks remain intact. Security audit
records for rejected requests remain intentional.

Creation retains the fresh actor, permission-assignment and role locks in the
caller's SQLx transaction through publication. The exact associated pack is
resolved again under its transaction-scoped namespace lock. Workflow sync uses
that same owning transaction for filesystem loading and registrar writes; it
never publishes through an independent pool transaction. Association supplies
`pack_refs` scoping independently of `managing_pack_id` and cannot confer
`pack_owner` eligibility.

New workflow files use private owned staging and no-clobber links. A pre-commit
failure or cancellation removes only files and empty directories created by
that request. A tracked, resource-admitted owner finishes SQL COMMIT after file
publication, even if the HTTP waiter disconnects; service shutdown joins it.
Only recognized ordinary PostgreSQL transaction-rejection SQLSTATEs permit file cleanup; fatal or unfamiliar database responses retain custody too. A transport failure with an unknown
commit outcome conservatively retains the publication and a private
`uncertain-commit.json` manifest in its `.workflow-publication-*` directory for
operator reconciliation. PostgreSQL and the filesystem do not form one atomic
transaction; a process/host loss or an uncertain commit requires reconciliation,
not an assumption that SQL rollback removed files.

## Operator-approved local pack sources

HTTP registration (single and batch), installation and staged download may read
API-host paths only for Access credentials with unconstrained `packs:install`
and only under canonical roots explicitly configured in
`packs.local_source_roots`. The default empty list disables local ingestion.
Candidate namespace authorization still applies. Scoped upload/remote deployers
must use upload or remote sources; deployment authority alone does not disclose
API-host files. Upload and remote staging do not require local-source approval.

For operator local registration, configure a dedicated, operator-owned source
mount, for example `packs: {local_source_roots: [/srv/attune-pack-sources]}`,
and register a pack directory beneath that mount using an operator credential.
Keep the mount and its ancestors immutable to untrusted writers while ingestion
runs; canonical path checks are not a sandbox against concurrent host filesystem
mutation. Do not approve `/`, home directories, or shared temporary roots.
Directory copying and release publication reject symlinks within pack trees.
Test runner entrypoints must be relative existing files resolving inside the
owned pack root; traversal, absolute paths and symlink escapes are rejected by
the executor as well as metadata validation.

The host-path dependency and environment-inspection endpoints also preflight
**every** requested path under these approved roots before reading any manifest,
examining environment paths or running version probes. Dependency inspection
requires global Access `packs:read`; environment detection requires global Access
`packs:install`. Neither grant replaces the approved-root policy. Inspection
rejects symlinked manifests, requirements/package files and environment paths.
`build-envs` detects API-host runtimes and environment presence; it does not
execute pack scripts or install dependencies.

Direct upload and remote/managed installation use their existing private candidate
staging and publication paths and do not need approved host source roots. If an
operator separately inspects an installed upload/remote pack through these
host-path endpoints, explicitly approve its operator-owned projection root. The
staged `download` → `dependencies` → `build-envs` → `register-batch` flow (including
`core.pack_api` callers using an operator Access token) requires an explicitly
approved service-owned staging root, currently `/tmp/attune-pack-downloads`, for
its returned `pack_path` values. Precreate that dedicated directory with service
ownership and permissions, protect its ancestors, and approve that directory
only, never all of `/tmp`. Caller `destination_dir`/`packs_base_dir` values do not
establish trust. No automatic trust is inferred from an ambient temporary path
or a client-supplied pack name. The UI/CLI direct install/upload flows do not call
these inspection endpoints or require that additional staging-root approval.
