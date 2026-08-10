# WYSIWYG Dashboard Builder MVP Requirements

## 1. Dashboard lifecycle
- Create dashboard metadata (`ref`, `label`, `description`, `scope`, `visibility`, `tags`, `enabled`, `is_default_home`)
- Edit dashboard metadata (`label`, `description`, `scope`, `visibility`, `tags`, `enabled`, `is_default_home`)
- Treat `ref` as immutable after create (rename = clone to new ref + delete old ref)
- Support draft vs publish workflow with an explicitly chosen model:
  - Option A (MVP default): save = publish, with ephemeral unsaved drafts + preview endpoint
  - Option B: persisted drafts with explicit publish action and draft state
- Clone existing dashboards with explicit reset rules (`ref` required new value, `is_default_home=false` on clone)
- Enforce single default-home dashboard per `(scope_type, scope_ref)`; setting default home must atomically clear prior default in same scope

## 2. Visual layout editor
- Grid canvas with drag/resize for cards
- Breakpoint-aware layout editing (`lg`, `sm` minimum)
- Add/remove/reorder cards with snap-to-grid behavior

## 3. Card editor
- Choose source, visualization type, title, and subtitle
- Field mapping UI for (`x_field`, `y_field`, `series_field`, `value_field`) based on source shape
- Per-card options for core visualization types (table/stat/timeseries/gauge)

## 4. Data source editor
- Add/edit/remove `data_sources`
- Source-type-specific parameter forms (prefer typed forms over raw JSON)
- Template-aware parameter inputs for filters (`{{ filters.* }}`) with autocomplete/validation

## 5. Filter/defaults editor
- Manage filter definitions and defaults (`timezone`, `time_window`, `refresh_seconds`)
- Validate that referenced filters exist in data source templates

## 6. Validation + preview
- Inline validation aligned with backend validation, with explicit gap handling for rules not currently enforced server-side
- Live preview against a dashboard data endpoint, including unsaved draft/spec preview support
- Clear display of source-level errors, partial/stale/empty states, truncation, and freshness metadata

## 7. YAML round-trip
- Read/write dashboard specs with deterministic output
- Include “View YAML” to show exact persisted representation
- Preserve stable key ordering and IDs to minimize git diff noise

## Non-functional requirements
- RBAC-aware UX (`dashboards:read/create/update/delete`)
- Optimistic concurrency/revision checks to prevent silent overwrite
- Backward-compatible loading/editing for existing dashboard specs
- Schema-driven forms where possible for future source types

## Must-have before implementation starts
- Define the authoring API contract first:
  - list/create/get/update/delete/clone dashboard endpoints
  - preview endpoint for unsaved dashboard specs/drafts
- Lock the draft/publish/revision model:
  - define revision semantics and publish behavior (save=publish vs persisted draft)
  - require `dashboard_version` snapshots for each persisted spec revision
  - define restore behavior as roll-forward (restoring revision N creates N+1, not decrementing revision)
  - define whether metadata-only edits create dashboard_version entries
- Clarify metadata/spec ownership and conflict rules:
  - identify which fields are metadata-owned vs spec-owned
  - define canonical normalization behavior on save
- Require lossless round-trip behavior:
  - preserve unknown fields, or open unsupported specs read-only with explicit warning
- Define source catalog contract as API-driven:
  - source availability (`available_now`/`partial`/`planned`)
  - source param schema and field/type hints for mapping UIs
- Define breakpoint/layout behavior explicitly:
  - placement defaults, collision handling, min/max card dimensions
  - position bounds validation (`x/y/w/h` constraints and `w <= breakpoint.columns`)
  - behavior for cross-breakpoint edits (independent per breakpoint vs propagation rules)
- Define RBAC matrix for dashboard actions:
  - read/create/update/delete/clone/publish/default-home by scope and visibility
  - map clone/publish to concrete existing authz actions (or define new actions if needed)
  - define permissions for scope reassignment and visibility changes

## Should-have in MVP
- Server-normalized YAML view (not only client-generated YAML)
- Explicit conflict UX on revision mismatch (reload/merge/overwrite choice)
- Deterministic handling of all source states in preview and runtime (`ok`, `empty`, `partial`, `stale`, `forbidden`, `invalid`, `error`)
- Clear clone semantics (what is copied vs reset)
- Controlled behavior for partial/planned source types in editor
- Preview telemetry surfacing (`meta.truncated`, `meta.authorization_mode`, `meta.freshness_mode`, `meta.aggregate_watermark`)

## Nice-to-have post-MVP
- Revision history browser and restore actions
- Visual diff between revisions
- Autosave and recovery for unsaved edits
- Undo/redo for layout and card editing
- Dashboard/card templates

## Acceptance criteria checklist
- [ ] Create, preview, save, reopen, and update a dashboard succeeds end-to-end.
- [ ] Unsaved edits can be previewed without publishing.
- [ ] Save/update enforces optimistic concurrency and returns a clear revision conflict.
- [ ] Revision creation semantics are deterministic and documented; persisted spec changes create a recoverable version history.
- [ ] YAML output is deterministic and matches persisted server representation.
- [ ] Existing valid dashboards can be edited without data loss, or are explicitly read-only if unsupported.
- [ ] Source forms and field mapping controls are driven by API-provided source contract metadata.
- [ ] Layout editing produces valid positions for required breakpoints with deterministic collision behavior.
- [ ] Preview and runtime rendering handle source-level `ok/empty/partial/stale/forbidden/invalid/error` states consistently.
- [ ] Preview surfaces truncation and freshness/watermark metadata where applicable.
- [ ] Setting `is_default_home` in a scope is atomic and never violates single-default-per-scope constraints.
- [ ] `ref` remains immutable after create; rename flow is clone + delete.
- [ ] RBAC is enforced by API and reflected in UI affordances for all dashboard actions.
