/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateWorkflowRequest } from "../models/CreateWorkflowRequest";
import type { PaginatedResponse_WorkflowSummary } from "../models/PaginatedResponse_WorkflowSummary";
import type { SaveWorkflowFileRequest } from "../models/SaveWorkflowFileRequest";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateWorkflowRequest } from "../models/UpdateWorkflowRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class WorkflowsService {
  /**
   * Save a workflow file to disk and sync it to the database
   * Writes a `{name}.workflow.yaml` file to `{packs_base_dir}/{pack_ref}/actions/workflows/`
   * and creates or updates the corresponding workflow_definition record in the database.
   * Also creates a companion action record so the workflow appears in action lists and palettes.
   * @returns any Workflow file saved and synced
   * @throws ApiError
   */
  public static saveWorkflowFile({
    packRef,
    requestBody,
  }: {
    /**
     * Pack reference identifier
     */
    packRef: string;
    requestBody: SaveWorkflowFileRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for workflow information
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Workflow definition
       */
      definition: Record<string, any>;
      /**
       * Workflow description
       */
      description?: string | null;
      /**
       * Workflow ID
       */
      id: number;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Output schema
       */
      out_schema: any | null;
      /**
       * Pack ID
       */
      pack: number;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Parameter schema (StackStorm-style with inline required/secret)
       */
      param_schema: any | null;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Workflow version
       */
      version: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/packs/{pack_ref}/workflow-files",
      path: {
        pack_ref: packRef,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Pack not found`,
        409: `Workflow with same ref already exists`,
        500: `Failed to write workflow file`,
      },
    });
  }
  /**
   * List workflows by pack reference
   * @returns PaginatedResponse_WorkflowSummary List of workflows for pack
   * @throws ApiError
   */
  public static listWorkflowsByPack({
    packRef,
    page,
    pageSize,
  }: {
    /**
     * Pack reference identifier
     */
    packRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_WorkflowSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/workflows",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        404: `Pack not found`,
      },
    });
  }
  /**
   * List all workflows with pagination and filtering
   * @returns PaginatedResponse_WorkflowSummary List of workflows
   * @throws ApiError
   */
  public static listWorkflows({
    page,
    pageSize,
    tags,
    search,
    packRef,
  }: {
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Filter by tag(s) - comma-separated list
     */
    tags?: string | null;
    /**
     * Search term for label/description (case-insensitive)
     */
    search?: string | null;
    /**
     * Filter by pack reference
     */
    packRef?: string | null;
  }): CancelablePromise<PaginatedResponse_WorkflowSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workflows",
      query: {
        page: page,
        page_size: pageSize,
        tags: tags,
        search: search,
        pack_ref: packRef,
      },
    });
  }
  /**
   * Create a new workflow
   * @returns any Workflow created successfully
   * @throws ApiError
   */
  public static createWorkflow({
    requestBody,
  }: {
    requestBody: CreateWorkflowRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for workflow information
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Workflow definition
       */
      definition: Record<string, any>;
      /**
       * Workflow description
       */
      description?: string | null;
      /**
       * Workflow ID
       */
      id: number;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Output schema
       */
      out_schema: any | null;
      /**
       * Pack ID
       */
      pack: number;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Parameter schema (StackStorm-style with inline required/secret)
       */
      param_schema: any | null;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Workflow version
       */
      version: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workflows",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Pack not found`,
        409: `Workflow with same ref already exists`,
      },
    });
  }
  /**
   * Get a single workflow by reference
   * @returns any Workflow details
   * @throws ApiError
   */
  public static getWorkflow({
    ref,
  }: {
    /**
     * Workflow reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Response DTO for workflow information
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Workflow definition
       */
      definition: Record<string, any>;
      /**
       * Workflow description
       */
      description?: string | null;
      /**
       * Workflow ID
       */
      id: number;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Output schema
       */
      out_schema: any | null;
      /**
       * Pack ID
       */
      pack: number;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Parameter schema (StackStorm-style with inline required/secret)
       */
      param_schema: any | null;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Workflow version
       */
      version: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workflows/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Workflow not found`,
      },
    });
  }
  /**
   * Update an existing workflow
   * @returns any Workflow updated successfully
   * @throws ApiError
   */
  public static updateWorkflow({
    ref,
    requestBody,
  }: {
    /**
     * Workflow reference identifier
     */
    ref: string;
    requestBody: UpdateWorkflowRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for workflow information
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Workflow definition
       */
      definition: Record<string, any>;
      /**
       * Workflow description
       */
      description?: string | null;
      /**
       * Workflow ID
       */
      id: number;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Output schema
       */
      out_schema: any | null;
      /**
       * Pack ID
       */
      pack: number;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Parameter schema (StackStorm-style with inline required/secret)
       */
      param_schema: any | null;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Workflow version
       */
      version: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/workflows/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Workflow not found`,
      },
    });
  }
  /**
   * Delete a workflow
   * @returns SuccessResponse Workflow deleted successfully
   * @throws ApiError
   */
  public static deleteWorkflow({
    ref,
  }: {
    /**
     * Workflow reference identifier
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/workflows/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Workflow not found`,
      },
    });
  }
  /**
   * Update a workflow file on disk and sync changes to the database
   * @returns any Workflow file updated and synced
   * @throws ApiError
   */
  public static updateWorkflowFile({
    ref,
    requestBody,
  }: {
    /**
     * Workflow reference identifier
     */
    ref: string;
    requestBody: SaveWorkflowFileRequest;
  }): CancelablePromise<{
    /**
     * Response DTO for workflow information
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Workflow definition
       */
      definition: Record<string, any>;
      /**
       * Workflow description
       */
      description?: string | null;
      /**
       * Workflow ID
       */
      id: number;
      /**
       * Human-readable label
       */
      label: string;
      /**
       * Output schema
       */
      out_schema: any | null;
      /**
       * Pack ID
       */
      pack: number;
      /**
       * Pack reference
       */
      pack_ref: string;
      /**
       * Parameter schema (StackStorm-style with inline required/secret)
       */
      param_schema: any | null;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Tags
       */
      tags: Array<string>;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * Workflow version
       */
      version: string;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/workflows/{ref}/file",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Workflow not found`,
        500: `Failed to write workflow file`,
      },
    });
  }
}
