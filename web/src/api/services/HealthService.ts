/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { HealthResponse } from "../models/HealthResponse";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class HealthService {
  /**
   * Platform health check endpoint.
   * @returns any Database and exact platform catalog are ready
   * @throws ApiError
   */
  public static health(): CancelablePromise<Record<string, any>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/health",
      errors: {
        503: `Platform is not ready`,
      },
    });
  }
  /**
   * Transitional content and coarse host-capability health.
   * Required-pack locks and candidate evidence replace this contract in issue #75.
   * @returns any Core content and coarse host capabilities are available
   * @throws ApiError
   */
  public static content(): CancelablePromise<Record<string, any>> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/health/content",
      errors: {
        503: `Content or coarse host capabilities are absent`,
      },
    });
  }
  /**
   * Detailed health check endpoint
   * Checks database connectivity and returns detailed status
   * @returns HealthResponse Service is healthy with details
   * @throws ApiError
   */
  public static healthDetailed(): CancelablePromise<HealthResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/health/detailed",
      errors: {
        503: `Service unavailable`,
      },
    });
  }
  /**
   * Liveness check endpoint
   * Returns 200 OK if the service process is alive
   * @returns any Service is alive
   * @throws ApiError
   */
  public static liveness(): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/health/live",
    });
  }
  /**
   * Readiness check endpoint
   * Returns 200 OK if the service is ready to accept requests
   * @returns any Service is ready
   * @throws ApiError
   */
  public static readiness(): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/health/ready",
      errors: {
        503: `Service not ready`,
      },
    });
  }
}
