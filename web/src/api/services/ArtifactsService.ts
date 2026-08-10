/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AllocateFileVersionByRefRequest } from "../models/AllocateFileVersionByRefRequest";
import type { AppendProgressRequest } from "../models/AppendProgressRequest";
import type { ArtifactClassification } from "../models/ArtifactClassification";
import type { ArtifactType } from "../models/ArtifactType";
import type { ArtifactVersionByRefUploadForm } from "../models/ArtifactVersionByRefUploadForm";
import type { ArtifactVersionUploadForm } from "../models/ArtifactVersionUploadForm";
import type { ArtifactVisibility } from "../models/ArtifactVisibility";
import type { CreateArtifactRequest } from "../models/CreateArtifactRequest";
import type { CreateFileVersionRequest } from "../models/CreateFileVersionRequest";
import type { CreateVersionJsonRequest } from "../models/CreateVersionJsonRequest";
import type { OwnerType } from "../models/OwnerType";
import type { PaginatedResponse_ArtifactSummary } from "../models/PaginatedResponse_ArtifactSummary";
import type { RetentionPolicyType } from "../models/RetentionPolicyType";
import type { SetDataRequest } from "../models/SetDataRequest";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateArtifactRequest } from "../models/UpdateArtifactRequest";
import type { Value } from "../models/Value";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class ArtifactsService {
  /**
   * List artifacts with pagination and optional filters
   * @returns PaginatedResponse_ArtifactSummary List of artifacts
   * @throws ApiError
   */
  public static listArtifacts({
    scope,
    owner,
    type,
    visibility,
    classification,
    execution,
    name,
    page,
    perPage,
  }: {
    /**
     * Filter by owner scope type
     */
    scope?: null | OwnerType;
    /**
     * Filter by owner identifier
     */
    owner?: string | null;
    /**
     * Filter by artifact type
     */
    type?: null | ArtifactType;
    /**
     * Filter by visibility
     */
    visibility?: null | ArtifactVisibility;
    /**
     * Filter by classification
     */
    classification?: null | ArtifactClassification;
    /**
     * Filter to artifacts that have at least one version produced by this execution
     */
    execution?: number | null;
    /**
     * Search by name (case-insensitive substring match)
     */
    name?: string | null;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Items per page
     */
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_ArtifactSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts",
      query: {
        scope: scope,
        owner: owner,
        type: type,
        visibility: visibility,
        classification: classification,
        execution: execution,
        name: name,
        page: page,
        per_page: perPage,
      },
    });
  }
  /**
   * Create a new artifact
   * @returns any Artifact created
   * @throws ApiError
   */
  public static createArtifact({
    requestBody,
  }: {
    requestBody: CreateArtifactRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for artifact information
     */
    data: {
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      data?: null | Value;
      /**
       * Description
       */
      description?: string | null;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Retention limit
       */
      retention_limit: number;
      /**
       * Retention policy
       */
      retention_policy: RetentionPolicyType;
      /**
       * Owner scope type
       */
      scope: OwnerType;
      /**
       * Size of the latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        409: `Artifact with same ref already exists`,
      },
    });
  }
  /**
   * Get a single artifact by ref
   * @returns any Artifact details
   * @throws ApiError
   */
  public static getArtifactByRef({
    ref,
  }: {
    /**
     * Artifact reference
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Response DTO for artifact information
     */
    data: {
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      data?: null | Value;
      /**
       * Description
       */
      description?: string | null;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Retention limit
       */
      retention_limit: number;
      /**
       * Retention policy
       */
      retention_policy: RetentionPolicyType;
      /**
       * Owner scope type
       */
      scope: OwnerType;
      /**
       * Size of the latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/ref/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Upsert an artifact by ref and allocate a file-backed version in one call.
   * If the artifact doesn't exist, it is created using the supplied metadata.
   * If it already exists, the execution link is updated (if provided).
   * Then a new file-backed version is allocated and the `file_path` is returned.
   *
   * The caller writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on the
   * shared volume — no HTTP upload needed.
   * @returns any File version allocated
   * @throws ApiError
   */
  public static allocateFileVersionByRef({
    ref,
    requestBody,
  }: {
    /**
     * Artifact reference (e.g. 'mypack.build_log')
     */
    ref: string;
    requestBody: AllocateFileVersionByRefRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts/ref/{ref}/versions/file",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request (non-file-backed artifact type)`,
      },
    });
  }
  /**
   * Upload a file version to an artifact identified by ref, creating the artifact if it does not
   * already exist.
   * This is the recommended way for actions to produce versioned file artifacts. The caller
   * provides the artifact ref and file content in a single multipart request. The server:
   *
   * 1. Looks up the artifact by `ref`.
   * 2. If not found, creates it using the metadata fields in the multipart body.
   * 3. If found, optionally updates the `execution` link to the current execution.
   * 4. Uploads the file bytes as a new version (version number is auto-assigned).
   *
   * **Multipart fields:**
   * - `file` (required) — the binary file content
   * - `ref` (required for creation) — artifact reference (ignored if artifact already exists)
   * - `scope` — owner scope: `system`, `pack`, `action`, `sensor`, `rule` (default: `action`)
   * - `owner` — owner identifier (default: empty string)
   * - `type` — artifact type: `file_text`, `file_image`, etc. (default: `file_text`)
   * - `visibility` — `public` or `private` (default: type-aware server default)
   * - `name` — human-readable name
   * - `description` — optional description
   * - `content_type` — MIME type (default: auto-detected from multipart or `application/octet-stream`)
   * - `execution` — execution ID to link this artifact to (updates existing artifacts too)
   * - `retention_policy` — `versions`, `days`, `hours`, `minutes` (default: `versions`)
   * - `retention_limit` — limit value (default: `10`)
   * - `created_by` — who created this version
   * - `meta` — JSON metadata for this version
   * @returns any Version created (artifact may have been created too)
   * @throws ApiError
   */
  public static uploadVersionByRef({
    ref,
    formData,
  }: {
    /**
     * Artifact reference (created if not found)
     */
    ref: string;
    formData: ArtifactVersionByRefUploadForm;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts/ref/{ref}/versions/upload",
      path: {
        ref: ref,
      },
      formData: formData,
      mediaType: "multipart/form-data",
      errors: {
        400: `Missing file field or invalid metadata`,
        413: `File too large`,
      },
    });
  }
  /**
   * Get a single artifact by ID
   * @returns any Artifact details
   * @throws ApiError
   */
  public static getArtifact({
    id,
  }: {
    /**
     * Artifact ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Response DTO for artifact information
     */
    data: {
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      data?: null | Value;
      /**
       * Description
       */
      description?: string | null;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Retention limit
       */
      retention_limit: number;
      /**
       * Retention policy
       */
      retention_policy: RetentionPolicyType;
      /**
       * Owner scope type
       */
      scope: OwnerType;
      /**
       * Size of the latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Update an existing artifact
   * @returns any Artifact updated
   * @throws ApiError
   */
  public static updateArtifact({
    id,
    requestBody,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    requestBody: UpdateArtifactRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for artifact information
     */
    data: {
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      data?: null | Value;
      /**
       * Description
       */
      description?: string | null;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Retention limit
       */
      retention_limit: number;
      /**
       * Retention policy
       */
      retention_policy: RetentionPolicyType;
      /**
       * Owner scope type
       */
      scope: OwnerType;
      /**
       * Size of the latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/artifacts/{id}",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Delete an artifact (cascades to all versions, including disk files)
   * @returns SuccessResponse Artifact deleted
   * @throws ApiError
   */
  public static deleteArtifact({
    id,
  }: {
    /**
     * Artifact ID
     */
    id: number;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/artifacts/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Set the full data payload on an artifact (replaces existing data).
   * Useful for resetting progress, updating metadata, or setting structured content.
   * @returns any Data set
   * @throws ApiError
   */
  public static setArtifactData({
    id,
    requestBody,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    requestBody: SetDataRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for artifact information
     */
    data: {
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      data?: null | Value;
      /**
       * Description
       */
      description?: string | null;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Retention limit
       */
      retention_limit: number;
      /**
       * Retention policy
       */
      retention_policy: RetentionPolicyType;
      /**
       * Owner scope type
       */
      scope: OwnerType;
      /**
       * Size of the latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/artifacts/{id}/data",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Download the latest version's content
   * @returns any Binary file content of latest version
   * @throws ApiError
   */
  public static downloadLatest({
    id,
  }: {
    /**
     * Artifact ID
     */
    id: number;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}/download",
      path: {
        id: id,
      },
      errors: {
        404: `Artifact not found or no versions`,
      },
    });
  }
  /**
   * Append an entry to a progress-type artifact's data array.
   * The entry is atomically appended to `artifact.data` (initialized as `[]` if null).
   * This is the primary mechanism for actions to stream progress updates.
   * @returns any Entry appended
   * @throws ApiError
   */
  public static appendProgress({
    id,
    requestBody,
  }: {
    /**
     * Artifact ID (must be progress type)
     */
    id: number;
    requestBody: AppendProgressRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for artifact information
     */
    data: {
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      data?: null | Value;
      /**
       * Description
       */
      description?: string | null;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Retention limit
       */
      retention_limit: number;
      /**
       * Retention policy
       */
      retention_policy: RetentionPolicyType;
      /**
       * Owner scope type
       */
      scope: OwnerType;
      /**
       * Size of the latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts/{id}/progress",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Artifact is not a progress type`,
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Stream the latest file-backed artifact version as Server-Sent Events.
   * The endpoint:
   * 1. Waits (up to ~30 s) for the file to appear on disk if it has been
   * allocated but not yet written by the worker.
   * 2. Once the file exists it sends the current content as an initial `content`
   * event, then tails the file every 500 ms, sending `append` events with new
   * bytes.
   * 3. When no new bytes have appeared for several consecutive checks **and** the
   * linked execution (if any) has reached a terminal status, it sends a `done`
   * event and the stream ends.
   * 4. If the client disconnects the stream is cleaned up automatically.
   *
   * **Event types** (SSE `event:` field):
   * - `content`  – full file content up to the current offset (sent once)
   * - `append`   – incremental bytes appended since the last event
   * - `waiting`  – file does not exist yet; sent periodically while waiting
   * - `done`     – no more data expected; stream will close
   * - `error`    – something went wrong; `data` contains a human-readable message
   * @returns any SSE stream of file content
   * @throws ApiError
   */
  public static streamArtifact({
    id,
  }: {
    /**
     * Artifact ID
     */
    id: number;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}/stream",
      path: {
        id: id,
      },
      errors: {
        401: `Unauthorized`,
        404: `Artifact not found or not file-backed`,
      },
    });
  }
  /**
   * List all versions for an artifact (without binary content)
   * @returns any List of versions
   * @throws ApiError
   */
  public static listVersions({
    id,
  }: {
    /**
     * Artifact ID
     */
    id: number;
  }): CancelablePromise<{
    data: Array<{
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number
       */
      version: number;
    }>;
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}/versions",
      path: {
        id: id,
      },
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Create a new version with JSON content
   * @returns any Version created
   * @throws ApiError
   */
  public static createVersionJson({
    id,
    requestBody,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    requestBody: CreateVersionJsonRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts/{id}/versions",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Create a new file-backed version (no file content in request).
   * This endpoint allocates a version number and computes a `file_path` on the
   * shared artifact volume. The caller (execution process) is expected to write
   * the file content directly to `$ATTUNE_ARTIFACTS_DIR/{file_path}` after
   * receiving the response. The worker finalizes `size_bytes` after execution.
   *
   * Only applicable to file-type artifacts (FileBinary, FileDatatable, FileText, Log).
   * @returns any File version allocated
   * @throws ApiError
   */
  public static createVersionFile({
    id,
    requestBody,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    requestBody: CreateFileVersionRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts/{id}/versions/file",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Artifact type is not file-based`,
        404: `Artifact not found`,
      },
    });
  }
  /**
   * Get the latest version's metadata and JSON content
   * @returns any Latest version
   * @throws ApiError
   */
  public static getLatestVersion({
    id,
  }: {
    /**
     * Artifact ID
     */
    id: number;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}/versions/latest",
      path: {
        id: id,
      },
      errors: {
        404: `Artifact not found or no versions`,
      },
    });
  }
  /**
   * Upload a binary file as a new version (multipart/form-data)
   * The file is sent as a multipart form field named `file`. Optional fields:
   * - `content_type`: MIME type override (auto-detected from filename if omitted)
   * - `meta`: JSON metadata string
   * - `created_by`: Creator identifier
   * @returns any File version created
   * @throws ApiError
   */
  public static uploadVersion({
    id,
    formData,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    formData: ArtifactVersionUploadForm;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/artifacts/{id}/versions/upload",
      path: {
        id: id,
      },
      formData: formData,
      mediaType: "multipart/form-data",
      errors: {
        400: `Missing file field`,
        404: `Artifact not found`,
        413: `File too large`,
      },
    });
  }
  /**
   * Get a specific version's metadata and JSON content (no binary)
   * @returns any Version details
   * @throws ApiError
   */
  public static getVersion({
    id,
    version,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    /**
     * Version number
     */
    version: number;
  }): CancelablePromise<{
    /**
     * Response DTO for an artifact version (without binary content)
     */
    data: {
      /**
       * Parent artifact ID
       */
      artifact: number;
      content_json?: null | Value;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Who created this version
       */
      created_by?: string | null;
      /**
       * Execution that produced this version (e.g., the execution that wrote
       * this log version). Per-version association — the parent artifact may
       * be linked to many executions across versions.
       */
      execution?: number | null;
      /**
       * Relative file path for disk-backed versions (from artifacts_dir root).
       * When present, the file content lives on the shared volume, not in the DB.
       */
      file_path?: string | null;
      /**
       * Version ID
       */
      id: number;
      meta?: null | Value;
      /**
       * Size of content in bytes
       */
      size_bytes?: number | null;
      /**
       * Version number (1-based)
       */
      version: number;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}/versions/{version}",
      path: {
        id: id,
        version: version,
      },
      errors: {
        404: `Artifact or version not found`,
      },
    });
  }
  /**
   * Delete a specific version by version number (including disk file if file-backed)
   * @returns SuccessResponse Version deleted
   * @throws ApiError
   */
  public static deleteVersion({
    id,
    version,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    /**
     * Version number
     */
    version: number;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/artifacts/{id}/versions/{version}",
      path: {
        id: id,
        version: version,
      },
      errors: {
        404: `Artifact or version not found`,
      },
    });
  }
  /**
   * Download the binary content of a specific version.
   * For file-backed versions, reads from the shared artifact volume on disk.
   * For DB-stored versions, reads from the BYTEA/JSON content column.
   * @returns any Binary file content
   * @throws ApiError
   */
  public static downloadVersion({
    id,
    version,
  }: {
    /**
     * Artifact ID
     */
    id: number;
    /**
     * Version number
     */
    version: number;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/artifacts/{id}/versions/{version}/download",
      path: {
        id: id,
        version: version,
      },
      errors: {
        404: `Artifact, version, or content not found`,
      },
    });
  }
  /**
   * List all artifacts for a given execution
   * @returns any List of artifacts for execution
   * @throws ApiError
   */
  public static listArtifactsByExecution({
    executionId,
  }: {
    /**
     * Execution ID
     */
    executionId: number;
  }): CancelablePromise<{
    data: Array<{
      /**
       * Classification used to distinguish runtime log artifacts from general artifacts.
       */
      classification: ArtifactClassification;
      /**
       * MIME content type
       */
      content_type?: string | null;
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Artifact ID
       */
      id: number;
      /**
       * Human-readable name
       */
      name?: string | null;
      /**
       * Owner identifier
       */
      owner: string;
      /**
       * Artifact reference
       */
      ref: string;
      /**
       * Owner scope
       */
      scope: OwnerType;
      /**
       * Size of latest version in bytes
       */
      size_bytes?: number | null;
      /**
       * Artifact type
       */
      type: ArtifactType;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Visibility level
       */
      visibility: ArtifactVisibility;
    }>;
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/executions/{execution_id}/artifacts",
      path: {
        execution_id: executionId,
      },
    });
  }
}
