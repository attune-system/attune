import type { CancelablePromise } from "./core/CancelablePromise";
import { OpenAPI } from "./core/OpenAPI";
import { request as __request } from "./core/request";

export type JsonValue =
  | string
  | number
  | boolean
  | null
  | { [key: string]: JsonValue }
  | JsonValue[];

export interface PaginationMeta {
  page: number;
  page_size: number;
  total_items?: number | null;
  total_pages?: number | null;
  has_previous: boolean;
  has_next: boolean;
}

export interface PaginatedApiResponse<T> {
  items: T[];
  pagination: PaginationMeta;
}

export interface ApiResponse<T> {
  data: T;
  message?: string | null;
}

export interface SuccessResponse {
  message: string;
}

export enum WorkQueueUpdateStrategy {
  IMMUTABLE = "immutable",
  REPLACE = "replace",
  MERGE_PATCH = "merge_patch",
}

export enum WorkQueueBatchMode {
  SINGLE = "single",
  BATCH = "batch",
}

export enum WorkQueueItemStatus {
  QUEUED = "queued",
  LEASED = "leased",
  RETRY = "retry",
  COMPLETED = "completed",
  FAILED = "failed",
  SKIPPED = "skipped",
  CANCELLED = "cancelled",
}

export enum ReferenceVisibility {
  PUBLIC = "public",
  PRIVATE = "private",
  RESTRICTED = "restricted",
}

export type NullableStringPatch =
  | {
      op: "set";
      value: string;
    }
  | {
      op: "clear";
    };

export interface WorkQueueSummary {
  id: number;
  ref: string;
  pack_ref?: string | null;
  is_adhoc: boolean;
  label: string;
  description?: string | null;
  enabled: boolean;
  accepting_new_items: boolean;
  dispatch_action_ref: string;
  reference_visibility: ReferenceVisibility;
  reference_allowed_pack_refs?: string[];
  created: string;
  updated: string;
}

export interface WorkQueueResponse extends WorkQueueSummary {
  pack?: number | null;
  dispatch_action?: number | null;
  default_priority: number;
  allow_pending_update: boolean;
  update_strategy: WorkQueueUpdateStrategy;
  batch_mode: WorkQueueBatchMode;
  item_schema: JsonValue;
  action_params: JsonValue;
  permission_set_refs?: string[] | null;
  config: JsonValue;
  resolved_dispatch_tuning?: {
    concurrency?: number | null;
    batch_size?: number | null;
  } | null;
}

export interface WorkQueueItemResponse {
  id: number;
  queue: number;
  queue_ref: string;
  item_key?: string | null;
  priority: number;
  status: WorkQueueItemStatus;
  payload: JsonValue;
  metadata: JsonValue;
  enqueue_source: string;
  requested_by_identity?: number | null;
  requested_by_execution?: number | null;
  requested_by_enforcement?: number | null;
  leased_execution?: number | null;
  lease_token?: string | null;
  lease_expires_at?: string | null;
  attempt_count: number;
  last_error?: JsonValue | null;
  ack_summary?: JsonValue | null;
  created: string;
  updated: string;
}

export interface CreateWorkQueueRequest {
  ref: string;
  pack_ref?: string | null;
  label: string;
  description?: string | null;
  enabled?: boolean;
  accepting_new_items?: boolean;
  dispatch_action_ref: string;
  default_priority?: number;
  allow_pending_update?: boolean;
  update_strategy?: WorkQueueUpdateStrategy;
  batch_mode?: WorkQueueBatchMode;
  item_schema?: JsonValue;
  action_params?: JsonValue;
  permission_set_refs?: string[] | null;
  config?: JsonValue;
  reference_visibility?: ReferenceVisibility | null;
  reference_allowed_pack_refs?: string[];
}

export interface UpdateWorkQueueRequest {
  label?: string;
  description?: NullableStringPatch;
  enabled?: boolean;
  accepting_new_items?: boolean;
  dispatch_action_ref?: string;
  default_priority?: number;
  allow_pending_update?: boolean;
  update_strategy?: WorkQueueUpdateStrategy;
  batch_mode?: WorkQueueBatchMode;
  item_schema?: JsonValue;
  action_params?: JsonValue;
  permission_set_refs?: string[] | null;
  config?: JsonValue;
  reference_visibility?: ReferenceVisibility | null;
  reference_allowed_pack_refs?: string[] | null;
}

export interface EnqueueWorkQueueItemRequest {
  item_key?: string | null;
  priority?: number | null;
  payload: JsonValue;
  metadata?: JsonValue;
}

export interface UpdateWorkQueueItemRequest {
  item_key?: NullableStringPatch;
  priority?: number;
  payload?: JsonValue;
  metadata?: JsonValue;
}

export interface WorkQueueItemJsonPathSelector {
  path: string;
  vars?: Record<string, JsonValue>;
}

export interface PreviewWorkQueueItemsRequest {
  selector: WorkQueueItemJsonPathSelector;
  limit?: number;
}

export enum WorkQueueItemBulkOperation {
  CANCEL = "cancel",
  PATCH_PAYLOAD = "patch_payload",
  REPRIORITIZE = "reprioritize",
}

export interface ApplyWorkQueueItemsRequest {
  selector: WorkQueueItemJsonPathSelector;
  operation: WorkQueueItemBulkOperation;
  payload_patch?: JsonValue | null;
  priority?: number | null;
  preview_limit?: number;
}

export interface PreviewWorkQueueItemsResponse {
  matched_count: number;
  preview_count: number;
  items: WorkQueueItemResponse[];
}

export interface ApplyWorkQueueItemsResponse {
  operation: WorkQueueItemBulkOperation;
  matched_count: number;
  affected_count: number;
  skipped_count: number;
  preview_count: number;
  items: WorkQueueItemResponse[];
}

export interface ListQueuesParams {
  enabled?: boolean;
  isAdhoc?: boolean;
  search?: string;
  referencingPackRef?: string;
  page?: number;
  pageSize?: number;
}

export interface ListQueueItemsParams {
  itemKey?: string;
  enqueueSource?: string;
  statuses?: WorkQueueItemStatus[];
  page?: number;
  pageSize?: number;
}

export const MUTABLE_PENDING_STATUSES = [
  WorkQueueItemStatus.QUEUED,
  WorkQueueItemStatus.RETRY,
] as const;

export class WorkQueuesService {
  public static listQueues({
    enabled,
    isAdhoc,
    search,
    referencingPackRef,
    page,
    pageSize,
  }: ListQueuesParams = {}): CancelablePromise<PaginatedApiResponse<WorkQueueSummary>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/queues",
      query: {
        enabled,
        is_adhoc: isAdhoc,
        search,
        referencing_pack_ref: referencingPackRef,
        page,
        per_page: pageSize,
      },
    });
  }

  public static getQueue({
    ref,
    referencingPackRef,
  }: {
    ref: string;
    referencingPackRef?: string;
  }): CancelablePromise<ApiResponse<WorkQueueResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/queues/{ref}",
      path: {
        ref,
      },
      query: {
        referencing_pack_ref: referencingPackRef,
      },
      errors: {
        404: "Queue not found",
      },
    });
  }

  public static createQueue({
    requestBody,
  }: {
    requestBody: CreateWorkQueueRequest;
  }): CancelablePromise<ApiResponse<WorkQueueResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/queues",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Validation error",
        403: "Insufficient permissions",
        404: "Dispatch action not found",
        409: "Queue already exists",
      },
    });
  }

  public static updateQueue({
    ref,
    requestBody,
  }: {
    ref: string;
    requestBody: UpdateWorkQueueRequest;
  }): CancelablePromise<ApiResponse<WorkQueueResponse>> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/queues/{ref}",
      path: {
        ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Validation error",
        403: "Queue is not editable",
        404: "Queue or action not found",
      },
    });
  }

  public static deleteQueue({ ref }: { ref: string }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/queues/{ref}",
      path: {
        ref,
      },
      errors: {
        403: "Queue is not deletable",
        404: "Queue not found",
      },
    });
  }

  public static listQueueItems({
    ref,
    itemKey,
    enqueueSource,
    statuses,
    page,
    pageSize,
  }: {
    ref: string;
  } & ListQueueItemsParams): CancelablePromise<PaginatedApiResponse<WorkQueueItemResponse>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/queues/{ref}/items",
      path: {
        ref,
      },
      query: {
        item_key: itemKey,
        enqueue_source: enqueueSource,
        statuses,
        page,
        per_page: pageSize,
      },
      errors: {
        404: "Queue not found",
      },
    });
  }

  public static previewQueueItemsBySelector({
    ref,
    requestBody,
  }: {
    ref: string;
    requestBody: PreviewWorkQueueItemsRequest;
  }): CancelablePromise<ApiResponse<PreviewWorkQueueItemsResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/queues/{ref}/items/query/preview",
      path: {
        ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Invalid selector",
        403: "Insufficient permissions",
        404: "Queue not found",
      },
    });
  }

  public static applyQueueItemsBySelector({
    ref,
    requestBody,
  }: {
    ref: string;
    requestBody: ApplyWorkQueueItemsRequest;
  }): CancelablePromise<ApiResponse<ApplyWorkQueueItemsResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/queues/{ref}/items/query/apply",
      path: {
        ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Invalid selector or operation",
        403: "Insufficient permissions",
        404: "Queue not found",
      },
    });
  }

  public static enqueueQueueItem({
    ref,
    requestBody,
  }: {
    ref: string;
    requestBody: EnqueueWorkQueueItemRequest;
  }): CancelablePromise<ApiResponse<WorkQueueItemResponse>> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/queues/{ref}/items",
      path: {
        ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Validation error",
        404: "Queue not found",
        409: "Pending item conflict",
      },
    });
  }

  public static updateQueueItem({
    ref,
    itemId,
    requestBody,
  }: {
    ref: string;
    itemId: number;
    requestBody: UpdateWorkQueueItemRequest;
  }): CancelablePromise<ApiResponse<WorkQueueItemResponse>> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/queues/{ref}/items/{item_id}",
      path: {
        ref,
        item_id: itemId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: "Validation error",
        404: "Queue item not found",
        409: "Queue item is not mutable",
      },
    });
  }

  public static deleteQueueItem({
    ref,
    itemId,
  }: {
    ref: string;
    itemId: number;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/queues/{ref}/items/{item_id}",
      path: {
        ref,
        item_id: itemId,
      },
      errors: {
        404: "Queue item not found",
        409: "Queue item is not mutable",
      },
    });
  }
}
