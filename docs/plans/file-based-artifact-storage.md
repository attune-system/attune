# File-Based Artifact Storage Plan

## Overview

Replace PostgreSQL BYTEA storage for file-type artifacts with a shared filesystem volume. Execution processes write artifact files directly to disk via paths assigned by the API; the API serves those files from disk on download. The database stores only metadata (path, size, content type) — no binary content for file-based artifacts.

**Motivation:**
- Eliminates PostgreSQL bloat from large binary artifacts
- Enables executions to write files incrementally (streaming logs, large outputs) without buffering in memory for an API upload
- Artifacts can be retained independently of execution records (executions are hypertables with 90-day retention)
- Decouples artifact lifecycle from execution lifecycle — artifacts created by one execution can be accessed by others or by external systems

## Artifact Type Classification

| Type | Storage | Notes |
|------|---------|-------|
| `FileBinary` | **Disk** (shared volume) | Binary files produced by executions |
| `FileDatatable` | **Disk** (shared volume) | Tabular data files (CSV, etc.) |
| `FileText` | **Disk** (shared volume) | Text files, logs |
| `Log` | **Disk** (shared volume) | Execution stdout/stderr logs |
| `Progress` | **DB** (`artifact.data` JSONB) | Small structured progress entries — unchanged |
| `Url` | **DB** (`artifact.data` JSONB) | URL references — unchanged |

## Directory Structure

```
/opt/attune/artifacts/           # artifacts_dir (configurable)
└── {artifact_ref_slug}/         # derived from artifact ref (globally unique)
    ├── v1.txt                   # version 1
    ├── v2.txt                   # version 2
    └── v3.txt                   # version 3
```

**Key decisions:**
- **No execution ID in the path.** Artifacts may outlive execution records (hypertable retention) and may be shared across executions or created externally.
- **Keyed by artifact ref.** The `ref` column has a unique index, making it a stable, globally unique identifier. Dots in refs become directory separators (e.g., `mypack.build_log` → `mypack/build_log/`).
- **Version files named `v{N}.{ext}`** where `N` is the version number from `next_artifact_version()` and `ext` is derived from `content_type`.

## End-to-End Flow

### Happy Path

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌────────────────┐
│  Worker   │────▶│Execution │────▶│   API    │────▶│  Shared Volume │
│  Service  │     │ Process  │     │  Service │     │  /opt/attune/  │
│           │     │(Py/Node/ │     │          │     │   artifacts/   │
│           │     │  Shell)  │     │          │     │                │
└──────────┘     └──────────┘     └──────────┘     └────────────────┘
     │                │                │                     │
     │  1. Start exec │                │                     │
     │  Set ATTUNE_   │                │                     │
     │  ARTIFACTS_DIR │                │                     │
     │───────────────▶│                │                     │
     │                │                │                     │
     │                │ 2. POST /api/v1/artifacts            │
     │                │   {ref, type, execution}             │
     │                │───────────────▶│                     │
     │                │                │ 3. Create artifact   │
     │                │                │    row in DB         │
     │                │                │                     │
     │                │◀───────────────│                     │
     │                │  {id, ref, ...}│                     │
     │                │                │                     │
     │                │ 4. POST /api/v1/artifacts/{id}/versions
     │                │   {content_type}                     │
     │                │───────────────▶│                     │
     │                │                │ 5. Create version    │
     │                │                │    row (file_path,   │
     │                │                │    no BYTEA content) │
     │                │                │    + mkdir on disk   │
     │                │◀───────────────│                     │
     │                │  {id, version, │                     │
     │                │   file_path}   │                     │
     │                │                │                     │
     │                │ 6. Write file to                     │
     │                │    $ATTUNE_ARTIFACTS_DIR/file_path   │
     │                │─────────────────────────────────────▶│
     │                │                │                     │
     │  7. Exec exits │                │                     │
     │◀───────────────│                │                     │
     │                                 │                     │
     │  8. Finalize: stat files,       │                     │
     │     update size_bytes in DB     │                     │
     │     (direct DB access)          │                     │
     │─────────────────────────────────┘                     │
     │                                                       │
     ▼                                                       │
  ┌──────────┐                                               │
  │  Client  │  9. GET /api/v1/artifacts/{id}/download       │
  │  (UI)    │──────────────────▶ API reads from disk ◀──────┘
  └──────────┘
```

### Step-by-Step

1. **Worker receives execution from MQ**, prepares `ExecutionContext`, sets `ATTUNE_ARTIFACTS_DIR` environment variable.
2. **Execution process** calls `POST /api/v1/artifacts` to create the artifact record (ref, type, execution ID, content_type).
3. **API** creates the `artifact` row in DB, returns the artifact ID.
4. **Execution process** calls `POST /api/v1/artifacts/{id}/versions` to create a new version. For file-type artifacts, the request body contains content_type and optional metadata — **no file content**.
5. **API** creates the `artifact_version` row with a computed `file_path` (e.g., `mypack/build_log/v1.txt`), `content` BYTEA left NULL. Creates the parent directory on disk. Returns version ID and `file_path`.
6. **Execution process** writes file content to `$ATTUNE_ARTIFACTS_DIR/{file_path}`. Can write incrementally (append, stream, etc.).
7. **Execution process exits.**
8. **Worker finalizes**: scans artifact versions linked to this execution, `stat()`s each file on disk, updates `artifact_version.size_bytes` and `artifact.size_bytes` in the DB via direct repository access.
9. **Client requests download**: API reads from `{artifacts_dir}/{file_path}` on disk and streams the response.

## Implementation Phases

### Phase 1: Configuration & Volume Infrastructure

**`crates/common/src/config.rs`**
- Add `artifacts_dir: String` to `Config` struct with default `/opt/attune/artifacts`
- Add `default_artifacts_dir()` function

**`config.development.yaml`**
- Add `artifacts_dir: ./artifacts`

**`config.docker.yaml`**
- Add `artifacts_dir: /opt/attune/artifacts`

**`docker-compose.yaml`**
- Add `artifacts_data` named volume
- Mount `artifacts_data:/opt/attune/artifacts` in: api (rw), all workers (rw), executor (rw for workflow activity logs)
- Add `ATTUNE__ARTIFACTS_DIR: /opt/attune/artifacts` to service environments where needed

### Phase 2: Database Schema Changes

**New migration: `migrations/20250101000011_artifact_file_storage.sql`**

```sql
-- Add file_path to artifact_version for disk-based storage
ALTER TABLE artifact_version ADD COLUMN IF NOT EXISTS file_path TEXT;

-- Index for finding versions by file_path (orphan cleanup)
CREATE INDEX IF NOT EXISTS idx_artifact_version_file_path
    ON artifact_version(file_path) WHERE file_path IS NOT NULL;

COMMENT ON COLUMN artifact_version.file_path IS
    'Relative path from artifacts_dir root for disk-stored content. '
    'When set, content BYTEA is NULL — file lives on shared volume.';
```

**`crates/common/src/models.rs`** — `artifact_version` module:
- Add `file_path: Option<String>` to `ArtifactVersion` struct
- Update `SELECT_COLUMNS` and `SELECT_COLUMNS_WITH_CONTENT` to include `file_path`

**`crates/common/src/repositories/artifact.rs`** — `ArtifactVersionRepository`:
- Add `file_path: Option<String>` to `CreateArtifactVersionInput`
- Wire `file_path` through the `create` query
- Add `update_size_bytes(executor, version_id, size_bytes)` method for worker finalization
- Add `find_file_versions_by_execution(executor, execution_id)` method — joins `artifact_version` → `artifact` on `artifact.execution` to find all file-based versions for an execution

### Phase 3: API Changes

#### Create Version Endpoint (modified)

`POST /api/v1/artifacts/{id}/versions` — currently `create_version_json`

Add a new endpoint or modify existing behavior:

**`POST /api/v1/artifacts/{id}/versions/file`** (new endpoint)
- Request body: `CreateFileVersionRequest { content_type: Option<String>, meta: Option<Value>, created_by: Option<String> }`
- **No file content in the request** — this is the key difference from `upload_version`
- API computes `file_path` from artifact ref + version number + content_type extension
- Creates `artifact_version` row with `file_path` set, `content` NULL
- Creates parent directory on disk: `{artifacts_dir}/{file_path_parent}/`
- Returns `ArtifactVersionResponse` **with `file_path` included**

**File path computation logic:**
```rust
fn compute_file_path(artifact_ref: &str, version: i32, content_type: &str) -> String {
    // "mypack.build_log" → "mypack/build_log"
    let ref_path = artifact_ref.replace('.', "/");
    let ext = extension_from_content_type(content_type);
    format!("{}/v{}.{}", ref_path, version, ext)
}
```

#### Download Endpoints (modified)

`GET /api/v1/artifacts/{id}/download` and `GET /api/v1/artifacts/{id}/versions/{v}/download`:
- If `artifact_version.file_path` is set:
  - Resolve absolute path: `{artifacts_dir}/{file_path}`
  - Verify file exists, return 404 if not
  - `stat()` the file for Content-Length header
  - Stream file content as response body
- If `file_path` is NULL:
  - Fall back to existing BYTEA/JSON content from DB (backward compatible)

#### Upload Endpoint (unchanged for now)

`POST /api/v1/artifacts/{id}/versions/upload` (multipart) — continues to store in DB BYTEA. This remains available for non-execution uploads (external systems, small files, etc.).

#### Response DTO Changes

**`crates/api/src/dto/artifact.rs`**:
- Add `file_path: Option<String>` to `ArtifactVersionResponse`
- Add `file_path: Option<String>` to `ArtifactVersionSummary`
- Add `CreateFileVersionRequest` DTO

### Phase 4: Worker Changes

#### Environment Variable Injection

**`crates/worker/src/executor.rs`** — `prepare_execution_context()`:
- Add `ATTUNE_ARTIFACTS_DIR` to the standard env vars block:
  ```rust
  env.insert("ATTUNE_ARTIFACTS_DIR".to_string(), self.artifacts_dir.clone());
  ```
- The `ActionExecutor` struct needs to hold the `artifacts_dir` value (sourced from config)

#### Post-Execution Finalization

**`crates/worker/src/executor.rs`** — after execution completes (success or failure):

```
async fn finalize_artifacts(&self, execution_id: i64) -> Result<()>
```

1. Query `artifact_version` rows joined through `artifact.execution = execution_id` where `file_path IS NOT NULL`
2. For each version with a `file_path`:
   - Resolve absolute path: `{artifacts_dir}/{file_path}`
   - `tokio::fs::metadata(path).await` to get file size
   - If file exists: update `artifact_version.size_bytes` via repository
   - If file doesn't exist: set `size_bytes = 0` (execution didn't produce the file)
3. For each parent artifact: update `artifact.size_bytes` to the latest version's `size_bytes`

This runs after every execution regardless of success/failure status, since even failed executions may have written partial artifacts.

#### Simplify Old ArtifactManager

**`crates/worker/src/artifacts.rs`**:
- The existing `ArtifactManager` is a standalone prototype disconnected from the DB-backed system. It can be simplified to only handle the `artifacts_dir` path resolution and directory creation, or removed entirely since the API now manages paths.
- Keep the struct as a thin wrapper if it's useful for the finalization logic, but remove the `store_logs`, `store_result`, `store_file` methods that duplicate what the API does.

### Phase 5: Retention & Cleanup

#### DB Trigger (existing, minor update)

The `enforce_artifact_retention` trigger fires `AFTER INSERT ON artifact_version` and deletes old version rows when the count exceeds the limit. This continues to work for row deletion. However, it **cannot** delete files on disk (triggers can't do filesystem I/O).

#### Orphan File Cleanup (new)

Add an async cleanup mechanism — either a periodic task in the worker/executor or a dedicated CLI command:

**`attune artifact cleanup`** (CLI) or periodic task:
1. Scan all files under `{artifacts_dir}/`
2. For each file, check if a matching `artifact_version.file_path` row exists
3. If no row exists (orphaned file), delete the file
4. Also delete empty directories

This handles:
- Files left behind after the retention trigger deletes version rows
- Files from crashed executions that created directories but whose version rows were cleaned up
- Manual DB cleanup scenarios

**Frequency:** Daily or on-demand via CLI. Orphaned files are not harmful (just wasted disk space), so aggressive cleanup isn't critical.

#### Artifact Deletion Endpoint

The existing `DELETE /api/v1/artifacts/{id}` cascades to `artifact_version` rows via FK. Enhance it to also delete files on disk:
- Before deleting the DB row, query all versions with `file_path IS NOT NULL`
- Delete each file from disk
- Then delete the DB row (cascades to version rows)
- Clean up empty parent directories

Similarly for `DELETE /api/v1/artifacts/{id}/versions/{v}`.

## Schema Summary

### artifact table (unchanged)

Existing columns remain. `size_bytes` continues to reflect the latest version's size (updated by worker finalization for file-based artifacts, updated by DB trigger for DB-stored artifacts).

### artifact_version table (modified)

| Column | Type | Notes |
|--------|------|-------|
| `id` | BIGSERIAL | PK |
| `artifact` | BIGINT | FK → artifact(id) ON DELETE CASCADE |
| `version` | INTEGER | Auto-assigned by `next_artifact_version()` |
| `content_type` | TEXT | MIME type |
| `size_bytes` | BIGINT | Set by worker finalization for file-based; set at insert for DB-stored |
| `content` | BYTEA | NULL for file-based artifacts; populated for DB-stored uploads |
| `content_json` | JSONB | For JSON content versions (unchanged) |
| **`file_path`** | **TEXT** | **NEW — relative path from `artifacts_dir`. When set, `content` is NULL** |
| `meta` | JSONB | Free-form metadata |
| `created_by` | TEXT | Who created this version |
| `created` | TIMESTAMPTZ | Immutable |

**Invariant:** Exactly one of `content`, `content_json`, or `file_path` should be non-NULL for a given version row.

## Files Changed

| File | Changes |
|------|---------|
| `crates/common/src/config.rs` | Add `artifacts_dir` field with default |
| `crates/common/src/models.rs` | Add `file_path` to `ArtifactVersion` |
| `crates/common/src/repositories/artifact.rs` | Wire `file_path` through create; add `update_size_bytes`, `find_file_versions_by_execution` |
| `crates/api/src/dto/artifact.rs` | Add `file_path` to version response DTOs; add `CreateFileVersionRequest` |
| `crates/api/src/routes/artifacts.rs` | New `create_version_file` endpoint; modify download endpoints for disk reads |
| `crates/api/src/state.rs` | No change needed — `config` already accessible via `AppState.config` |
| `crates/worker/src/executor.rs` | Inject `ATTUNE_ARTIFACTS_DIR` env var; add `finalize_artifacts()` post-execution |
| `crates/worker/src/service.rs` | Pass `artifacts_dir` config to `ActionExecutor` |
| `crates/worker/src/artifacts.rs` | Simplify or remove old `ArtifactManager` |
| `migrations/20250101000011_artifact_file_storage.sql` | Add `file_path` column to `artifact_version` |
| `config.development.yaml` | Add `artifacts_dir: ./artifacts` |
| `config.docker.yaml` | Add `artifacts_dir: /opt/attune/artifacts` |
| `docker-compose.yaml` | Add `artifacts_data` volume; mount in api + worker services |

## Environment Variables

| Variable | Set By | Available To | Value |
|----------|--------|--------------|-------|
| `ATTUNE_ARTIFACTS_DIR` | Worker | Execution process | Absolute path to artifacts volume (e.g., `/opt/attune/artifacts`) |
| `ATTUNE__ARTIFACTS_DIR` | Docker Compose | API / Worker services | Config override for `artifacts_dir` |

## Backward Compatibility

- **Existing DB-stored artifacts continue to work.** Download endpoints check `file_path` first, fall back to BYTEA/JSON content.
- **Existing multipart upload endpoint unchanged.** External systems can still upload small files via `POST /artifacts/{id}/versions/upload` — those go to DB as before.
- **Progress and URL artifacts unchanged.** They don't use `artifact_version` content at all.
- **No data migration needed.** Existing artifacts have `file_path = NULL` and continue to serve from DB.

## Future Considerations

- **External object storage (S3/MinIO):** The `file_path` abstraction makes it straightforward to swap the local filesystem for S3 later — the path becomes an object key, and the download endpoint proxies or redirects.
- **Streaming writes:** With disk-based storage, a future enhancement could allow the API to stream large file uploads directly to disk instead of buffering in memory.
- **Artifact garbage collection:** The orphan cleanup could be integrated into the executor's periodic maintenance loop alongside execution timeout monitoring.
- **Cross-execution artifact access:** Since artifacts are keyed by ref (not execution ID), a future enhancement could let actions declare artifact dependencies, and the worker could resolve and mount those paths.