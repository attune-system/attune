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
remains disabled and missing admissions fail closed. Existing workflow, rule and
trigger update routes retain their current IAM credential contracts; sensor
metadata update remains Access-only. Dedicated service operations retain their
own typed credential contracts.

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
