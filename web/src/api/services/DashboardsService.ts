/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_DashboardMetadataResponse } from "../models/ApiResponse_DashboardMetadataResponse";
import type { ApiResponse_DashboardSourceCatalogResponse } from "../models/ApiResponse_DashboardSourceCatalogResponse";
import type { ApiResponse_Vec_DashboardListItemResponse } from "../models/ApiResponse_Vec_DashboardListItemResponse";
import type { CloneDashboardRequest } from "../models/CloneDashboardRequest";
import type { CreateDashboardRequest } from "../models/CreateDashboardRequest";
import type { DashboardDataRequest } from "../models/DashboardDataRequest";
import type { DashboardDataResponse } from "../models/DashboardDataResponse";
import type { PreviewDashboardRequest } from "../models/PreviewDashboardRequest";
import type { SuccessResponse } from "../models/SuccessResponse";
import type { UpdateDashboardRequest } from "../models/UpdateDashboardRequest";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class DashboardsService {
  /**
   * @returns ApiResponse_Vec_DashboardListItemResponse Visible dashboard summaries
   * @throws ApiError
   */
  public static listDashboards(): CancelablePromise<ApiResponse_Vec_DashboardListItemResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/dashboards",
      errors: {
        401: `Unauthorized`,
      },
    });
  }
  /**
   * @returns ApiResponse_DashboardMetadataResponse Dashboard created successfully
   * @throws ApiError
   */
  public static createDashboard({
    requestBody,
  }: {
    requestBody: CreateDashboardRequest;
  }): CancelablePromise<ApiResponse_DashboardMetadataResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/dashboards",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        403: `Forbidden`,
        409: `Dashboard with same ref already exists in the target scope`,
        422: `Dashboard spec validation failed`,
      },
    });
  }
  /**
   * @returns DashboardDataResponse Dashboard preview data envelope
   * @throws ApiError
   */
  public static previewDashboard({
    requestBody,
  }: {
    requestBody: PreviewDashboardRequest;
  }): CancelablePromise<DashboardDataResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/dashboards/preview",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        403: `Forbidden`,
        422: `Dashboard spec validation failed`,
      },
    });
  }
  /**
   * @returns ApiResponse_DashboardSourceCatalogResponse Dashboard source contract catalog
   * @throws ApiError
   */
  public static getDashboardSourceCatalog(): CancelablePromise<ApiResponse_DashboardSourceCatalogResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/dashboards/source-catalog",
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
      },
    });
  }
  /**
   * @returns ApiResponse_DashboardMetadataResponse Dashboard metadata
   * @throws ApiError
   */
  public static getDashboard({
    ref,
  }: {
    /**
     * Dashboard reference identifier
     */
    ref: string;
  }): CancelablePromise<ApiResponse_DashboardMetadataResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/dashboards/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        400: `Invalid dashboard ref`,
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Dashboard not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_DashboardMetadataResponse Dashboard updated successfully
   * @throws ApiError
   */
  public static updateDashboard({
    ref,
    requestBody,
  }: {
    /**
     * Dashboard reference identifier
     */
    ref: string;
    requestBody: UpdateDashboardRequest;
  }): CancelablePromise<ApiResponse_DashboardMetadataResponse> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/dashboards/{ref}",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        403: `Forbidden or pack-managed dashboard`,
        404: `Dashboard not found`,
        409: `Revision mismatch or scope conflict`,
        422: `Dashboard spec validation failed`,
      },
    });
  }
  /**
   * @returns SuccessResponse Dashboard deleted successfully
   * @throws ApiError
   */
  public static deleteDashboard({
    ref,
  }: {
    /**
     * Dashboard reference identifier
     */
    ref: string;
  }): CancelablePromise<SuccessResponse> {
    return __request(OpenAPI, {
      method: "DELETE",
      url: "/api/v1/dashboards/{ref}",
      path: {
        ref: ref,
      },
      errors: {
        401: `Unauthorized`,
        403: `Forbidden or pack-managed dashboard`,
        404: `Dashboard not found`,
      },
    });
  }
  /**
   * @returns ApiResponse_DashboardMetadataResponse Dashboard cloned successfully
   * @throws ApiError
   */
  public static cloneDashboard({
    ref,
    requestBody,
  }: {
    /**
     * Dashboard reference identifier
     */
    ref: string;
    requestBody: CloneDashboardRequest;
  }): CancelablePromise<ApiResponse_DashboardMetadataResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/dashboards/{ref}/clone",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Dashboard not found`,
        409: `Dashboard with same ref already exists in the target scope`,
        422: `Dashboard spec validation failed`,
      },
    });
  }
  /**
   * @returns DashboardDataResponse Dashboard source data envelope
   * @throws ApiError
   */
  public static getDashboardData({
    ref,
    requestBody,
  }: {
    ref: string;
    requestBody: DashboardDataRequest;
  }): CancelablePromise<DashboardDataResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/dashboards/{ref}/data",
      path: {
        ref: ref,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid request`,
        401: `Unauthorized`,
        403: `Forbidden`,
        404: `Dashboard not found`,
      },
    });
  }
}
