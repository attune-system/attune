/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CreateKeyRequest } from "../models/CreateKeyRequest";
import type { i64 } from "../models/i64";
import type { OwnerType } from "../models/OwnerType";
import type { PaginatedResponse_KeySummary } from "../models/PaginatedResponse_KeySummary";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateKeyRequest } from "../models/UpdateKeyRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class SecretsService {
  /**
   * List all keys with pagination and optional filters (values redacted)
   * @returns PaginatedResponse_KeySummary List of keys (values redacted)
   * @throws ApiError
   */
  public static listKeys({
    ownerType,
    owner,
    page,
    perPage,
  }: {
    /**
     * Filter by owner type
     */
    ownerType?: null | OwnerType;
    /**
     * Filter by owner string
     */
    owner?: string | null;
    /**
     * Page number (1-indexed)
     */
    page?: number;
    /**
     * Items per page
     */
    perPage?: number;
  }): CancelablePromise<PaginatedResponse_KeySummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/keys",
      query: {
        owner_type: ownerType,
        owner: owner,
        page: page,
        per_page: perPage,
      },
    });
  }
  /**
   * Create a new key/secret
   * @returns any Key created successfully
   * @throws ApiError
   */
  public static createKey({
    requestBody,
  }: {
    requestBody: CreateKeyRequest;
  }): CancelablePromise<{
    /**
     * Full key response with all details (value redacted in list views)
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Whether the value is encrypted
       */
      encrypted: boolean;
      /**
       * Unique key ID
       */
      id: i64;
      /**
       * Key identifier within its owner scope
       */
      local_ref: string;
      /**
       * Human-readable name
       */
      name: string;
      /**
       * Authoritative owner reference
       */
      owner?: string | null;
      owner_action?: null | i64;
      /**
       * Owner action reference
       */
      owner_action_ref?: string | null;
      owner_identity?: null | i64;
      owner_pack?: null | i64;
      /**
       * Owner pack reference
       */
      owner_pack_ref?: string | null;
      owner_sensor?: null | i64;
      /**
       * Owner sensor reference
       */
      owner_sensor_ref?: string | null;
      /**
       * Type of owner
       */
      owner_type: OwnerType;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * The secret value (decrypted if encrypted). Can be a string, object, array, number, or boolean.
       */
      value: any;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/keys",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        409: `Key with same ref already exists`,
      },
    });
  }
  /**
   * Get a single key by reference (includes decrypted value)
   * @returns any Key details with decrypted value
   * @throws ApiError
   */
  public static getKey({
    ref,
  }: {
    /**
     * Key reference identifier
     */
    ref: string;
  }): CancelablePromise<{
    /**
     * Full key response with all details (value redacted in list views)
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Whether the value is encrypted
       */
      encrypted: boolean;
      /**
       * Unique key ID
       */
      id: i64;
      /**
       * Key identifier within its owner scope
       */
      local_ref: string;
      /**
       * Human-readable name
       */
      name: string;
      /**
       * Authoritative owner reference
       */
      owner?: string | null;
      owner_action?: null | i64;
      /**
       * Owner action reference
       */
      owner_action_ref?: string | null;
      owner_identity?: null | i64;
      owner_pack?: null | i64;
      /**
       * Owner pack reference
       */
      owner_pack_ref?: string | null;
      owner_sensor?: null | i64;
      /**
       * Owner sensor reference
       */
      owner_sensor_ref?: string | null;
      /**
       * Type of owner
       */
      owner_type: OwnerType;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * The secret value (decrypted if encrypted). Can be a string, object, array, number, or boolean.
       */
      value: any;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/keys/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Key not found`,
      },
    });
  }
  /**
   * Update an existing key/secret
   * @returns any Key updated successfully
   * @throws ApiError
   */
  public static updateKey({
    ref,
    requestBody,
  }: {
    /**
     * Key reference identifier
     */
    ref: string;
    requestBody: UpdateKeyRequest;
  }): CancelablePromise<{
    /**
     * Full key response with all details (value redacted in list views)
     */
    data: {
      /**
       * Creation timestamp
       */
      created: string;
      /**
       * Whether the value is encrypted
       */
      encrypted: boolean;
      /**
       * Unique key ID
       */
      id: i64;
      /**
       * Key identifier within its owner scope
       */
      local_ref: string;
      /**
       * Human-readable name
       */
      name: string;
      /**
       * Authoritative owner reference
       */
      owner?: string | null;
      owner_action?: null | i64;
      /**
       * Owner action reference
       */
      owner_action_ref?: string | null;
      owner_identity?: null | i64;
      owner_pack?: null | i64;
      /**
       * Owner pack reference
       */
      owner_pack_ref?: string | null;
      owner_sensor?: null | i64;
      /**
       * Owner sensor reference
       */
      owner_sensor_ref?: string | null;
      /**
       * Type of owner
       */
      owner_type: OwnerType;
      /**
       * Unique reference identifier
       */
      ref: string;
      /**
       * Last update timestamp
       */
      updated: string;
      /**
       * The secret value (decrypted if encrypted). Can be a string, object, array, number, or boolean.
       */
      value: any;
    };
    /**
     * Optional message
     */
    message?: string | null;
  }> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/keys/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Validation error`,
        404: `Key not found`,
      },
    });
  }
  /**
   * Delete a key/secret
   * @returns SuccessResponse Key deleted successfully
   * @throws ApiError
   */
  public static deleteKey({
    ref,
  }: {
    /**
     * Key reference identifier
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/keys/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Key not found`,
      },
    });
  }
}
