/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_TraceReportResponse } from "../models/ApiResponse_TraceReportResponse";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class TracesService {
  /**
   * @returns ApiResponse_TraceReportResponse Trace activity report
   * @throws ApiError
   */
  public static getTraceReport({
    traceTag,
  }: {
    /**
     * Exact trace tag to report
     */
    traceTag: string;
  }): CancelablePromise<ApiResponse_TraceReportResponse> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/traces/{trace_tag}",
      path: {
        trace_tag: traceTag,
      },
      errors: {
        400: `Invalid trace tag`,
        401: `Unauthorized`,
        403: `Insufficient permissions`,
      },
    });
  }
}
