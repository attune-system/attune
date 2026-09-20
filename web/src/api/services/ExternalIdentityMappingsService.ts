/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_ExternalIdentityMappingResponse } from "../models/ApiResponse_ExternalIdentityMappingResponse";
import type { ApiResponse_SuccessResponse } from "../models/ApiResponse_SuccessResponse";
import type { CreateExternalIdentityMappingRequest } from "../models/CreateExternalIdentityMappingRequest";
import type { PaginatedResponse_ExternalIdentityMappingResponse } from "../models/PaginatedResponse_ExternalIdentityMappingResponse";
import type { UpdateExternalIdentityMappingRequest } from "../models/UpdateExternalIdentityMappingRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class ExternalIdentityMappingsService {
  /**
   * @returns PaginatedResponse_ExternalIdentityMappingResponse Mappings
   * @throws ApiError
   */
  public static listExternalIdentityMappings({
    integrationIdentity,
    page,
    pageSize,
  }: {
    /**
     * Integration identity ID
     */
    integrationIdentity: number;
    /**
     * Page number (1-based)
     */
    page?: number;
    /**
     * Number of items per page
     */
    pageSize?: number;
  }): CancelablePromise<PaginatedResponse_ExternalIdentityMappingResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/identities/{integration_identity}/external-identity-mappings",
      path: {
        integration_identity: integrationIdentity,
      },
      query: {
        page: page,
        page_size: pageSize,
      },
      errors: {
        401: `Unauthorized`,
        403: `Insufficient identity read permission`,
        404: `Integration identity not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_ExternalIdentityMappingResponse Mapping created
   * @throws ApiError
   */
  public static createExternalIdentityMapping({
    integrationIdentity,
    requestBody,
  }: {
    /**
     * Integration identity ID
     */
    integrationIdentity: number;
    requestBody: CreateExternalIdentityMappingRequest;
  }): CancelablePromise<ApiResponse_ExternalIdentityMappingResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/identities/{integration_identity}/external-identity-mappings",
      path: {
        integration_identity: integrationIdentity,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        401: `Unauthorized`,
        403: `Insufficient identity administration permission`,
        404: `Integration identity not found`,
        409: `Mapping already exists`,
        422: `Invalid mapping`,
      },
    });
  }
  /**
   * @returns ApiResponse_ExternalIdentityMappingResponse Mapping
   * @throws ApiError
   */
  public static getExternalIdentityMapping({
    integrationIdentity,
    mappingId,
  }: {
    /**
     * Integration identity ID
     */
    integrationIdentity: number;
    /**
     * Mapping ID
     */
    mappingId: number;
  }): CancelablePromise<ApiResponse_ExternalIdentityMappingResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
      path: {
        integration_identity: integrationIdentity,
        mapping_id: mappingId,
      },
      errors: {
        401: `Unauthorized`,
        403: `Insufficient identity read permission`,
        404: `Identity or mapping not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_ExternalIdentityMappingResponse Mapping updated
   * @throws ApiError
   */
  public static updateExternalIdentityMapping({
    integrationIdentity,
    mappingId,
    requestBody,
  }: {
    /**
     * Integration identity ID
     */
    integrationIdentity: number;
    /**
     * Mapping ID
     */
    mappingId: number;
    requestBody: UpdateExternalIdentityMappingRequest;
  }): CancelablePromise<ApiResponse_ExternalIdentityMappingResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
      path: {
        integration_identity: integrationIdentity,
        mapping_id: mappingId,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        401: `Unauthorized`,
        403: `Insufficient identity administration permission`,
        404: `Identity or mapping not found`,
        409: `Mapping already exists`,
        422: `Invalid mapping`,
      },
    });
  }
  /**
   * @returns ApiResponse_SuccessResponse Mapping deleted
   * @throws ApiError
   */
  public static deleteExternalIdentityMapping({
    integrationIdentity,
    mappingId,
  }: {
    /**
     * Integration identity ID
     */
    integrationIdentity: number;
    /**
     * Mapping ID
     */
    mappingId: number;
  }): CancelablePromise<ApiResponse_SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}",
      path: {
        integration_identity: integrationIdentity,
        mapping_id: mappingId,
      },
      errors: {
        401: `Unauthorized`,
        403: `Insufficient identity administration permission`,
        404: `Identity or mapping not found`,
      },
    });
  }
}
