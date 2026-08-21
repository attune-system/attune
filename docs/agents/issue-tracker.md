# Issue tracker: Local SQLite

Issues for this repository are stored in the SQLite database at `.scratch/issues.db`.

Do not manipulate the database directly. Use the `issues` CLI for all issue operations.

## When a skill says "publish to the issue tracker"

Create an issue:

    issues create --title "<title>" --body-file "<body-path>"

## When a skill says "fetch the relevant ticket"

Read an issue:

    issues get <id>

## When a skill says "apply the <role> triage label"

    issues label <id> add <role>

Roles are the canonical triage labels below; the string used is the label name itself.

## Commands

List issues:

    issues list
    issues list --status ready-for-agent
    issues list --label needs-triage

Update an issue (status, title, or body):

    issues update <id> --status <status>
    issues update <id> --title "<title>"
    issues update <id> --body "<body>"
    issues update <id> --body-file "<path>"
    issues update <id> --append-body "<text>"

Add/remove labels:

    issues label <id> add <label>
    issues label <id> remove <label>

Close an issue (optionally with a resolution comment):

    issues close <id>
    issues close <id> --comment "<resolution>"

Add a comment:

    issues comment <id> --body "<text>"
    issues comment <id> --body-file "<path>"

Add/remove a blocking dependency:

    issues depends add <issue-id> --on <dependency-id>
    issues depends remove <issue-id> --on <dependency-id>

Attach/detach an issue to a parent (membership, not a blocker):

    issues attach <issue-id> --parent <map-id>
    issues detach <issue-id> --parent <map-id>

## Wayfinding operations

Used by `/wayfinder`.

- **Map**: an issue labelled `wayfinder:map`. The map body holds the Destination / Notes / Decisions-so-far / Not yet specified / Out of scope sections.
- **Child ticket**: an issue labelled `wayfinder:<type>` (`research`/`prototype`/`grilling`/`task`) attached to the map issue with `issues attach <id> --parent <map-id>`. Membership is not a blocker: the map staying open never blocks its children.
- **Blocking**: an edge added with `issues depends add <issue-id> --on <dependency-id>`. A ticket is unblocked when every issue it depends on is closed.
- **Frontier**: `issues frontier` (optionally `--label wayfinder:research` or `--map <map-id>`). Open, unblocked, parentless issues, first by id wins. Bare `frontier` excludes issues that have a parent, so a still-open map does not sit in the way.
- **Blocked**: `issues blocked` lists open issues and which dependency is blocking each.
- **Claim**: `issues update <id> --status in-progress` before any work.
- **Resolve**: append the answer as a comment with `issues comment <id> --body "<answer>"`, then close with `issues close <id> --comment "<resolution>"`, and append a context pointer to the map's Decisions-so-far with `issues update <map-id> --append-body-file "<pointer-path>"`.

## Agent grab queue

Open issues labelled `ready-for-agent` are fully specified and ready for an AFK agent:

    issues ready

## Canonical statuses

- open
- in-progress
- blocked
- closed

## Triage labels

- needs-triage
- needs-info
- ready-for-agent
- ready-for-human
- wontfix
