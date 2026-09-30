/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ApiResponse_BuildInfo } from "../models/ApiResponse_BuildInfo";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class InfoService {
  /**
   * @returns ApiResponse_BuildInfo Build identity of the responding API process
   * @throws ApiError
   */
  public static getInfo(): CancelablePromise<ApiResponse_BuildInfo> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/info",
    });
  }
}
