/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_RetentionConfig } from "../models/ApiResponse_RetentionConfig";
import type { RetentionConfig } from "../models/RetentionConfig";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class RetentionService {
  /**
   * Get runtime retention configuration.
   * @returns ApiResponse_RetentionConfig Runtime retention configuration
   * @throws ApiError
   */
  public static getRetentionConfig(): CancelablePromise<ApiResponse_RetentionConfig> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/retention-config",
      errors: {
        401: `Unauthorized`,
        403: `Forbidden`,
        500: `Internal server error`,
      },
    });
  }
  /**
   * Update runtime retention configuration.
   * @returns ApiResponse_RetentionConfig Runtime retention configuration updated
   * @throws ApiError
   */
  public static updateRetentionConfig({
    requestBody,
  }: {
    requestBody: RetentionConfig;
  }): CancelablePromise<ApiResponse_RetentionConfig> {
    return __request(OpenAPI, {
      method: "PUT",
      url: "/api/v1/retention-config",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        400: `Invalid retention configuration`,
        401: `Unauthorized`,
        403: `Forbidden`,
        500: `Internal server error`,
      },
    });
  }
}
