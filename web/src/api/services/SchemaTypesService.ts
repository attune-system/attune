/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_SchemaContractResponse } from "../models/ApiResponse_SchemaContractResponse";
import type { ApiResponse_SchemaTypeResponse } from "../models/ApiResponse_SchemaTypeResponse";
import type { PaginatedResponse_SchemaTypeResponse } from "../models/PaginatedResponse_SchemaTypeResponse";
import type { SchemaPreviewRequest } from "../models/SchemaPreviewRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class SchemaTypesService {
  /**
   * @returns PaginatedResponse_SchemaTypeResponse Installed type metadata for one pack
   * @throws ApiError
   */
  public static listPackSchemaTypes({
    packRef,
    page,
    pageSize,
    q,
  }: {
    packRef: string;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
    /**
     * Keyword query. Whitespace-separated tokens are AND-matched against
     * the catalog item's discovery fields.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_SchemaTypeResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/packs/{pack_ref}/schema-types",
      path: {
        pack_ref: packRef,
      },
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
    });
  }
  /**
   * @returns PaginatedResponse_SchemaTypeResponse Installed, non-sensitive type metadata
   * @throws ApiError
   */
  public static listSchemaTypes({
    page,
    pageSize,
    q,
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
     * Keyword query. Whitespace-separated tokens are AND-matched against
     * the catalog item's discovery fields.
     */
    q?: string | null;
  }): CancelablePromise<PaginatedResponse_SchemaTypeResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/schema-types",
      query: {
        page: page,
        page_size: pageSize,
        q: q,
      },
    });
  }
  /**
   * @returns ApiResponse_SchemaContractResponse Authoring rendering preview; not an authoritative assignment
   * @throws ApiError
   */
  public static previewSchema({
    requestBody,
  }: {
    requestBody: SchemaPreviewRequest;
  }): CancelablePromise<ApiResponse_SchemaContractResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/schema-types/preview",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid schema`,
      },
    });
  }
  /**
   * @returns ApiResponse_SchemaTypeResponse Authored named type definition
   * @throws ApiError
   */
  public static getSchemaType({
    ref,
  }: {
    ref: string;
  }): CancelablePromise<ApiResponse_SchemaTypeResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/schema-types/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        404: `Type not installed`,
      },
    });
  }
  /**
   * @returns ApiResponse_SchemaContractResponse Resolved named-type rendering contract
   * @throws ApiError
   */
  public static inspectSchemaType({
    ref,
  }: {
    ref: string;
  }): CancelablePromise<ApiResponse_SchemaContractResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/schema-types/{ref}/inspect",
      path: {
        ref: ref,
      },
    });
  }
}
