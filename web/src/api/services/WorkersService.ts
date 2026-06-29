/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CordonWorkerRequest } from "../models/CordonWorkerRequest";
import type { PaginatedResponse_WorkerSummary } from "../models/PaginatedResponse_WorkerSummary";
import type { WorkerHealthState } from "../models/WorkerHealthState";
import type { WorkerRole } from "../models/WorkerRole";
import type { WorkerStatus } from "../models/WorkerStatus";
import type { WorkerSummary } from "../models/WorkerSummary";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class WorkersService {
  /**
   * @returns PaginatedResponse_WorkerSummary List workers with runtime support and current load
   * @throws ApiError
   */
  public static listWorkers({
    page,
    pageSize,
    role,
    status,
    cordoned,
    healthState,
  }: {
    page?: number;
    pageSize?: number;
    role?: null | WorkerRole;
    status?: null | WorkerStatus;
    cordoned?: boolean | null;
    healthState?: null | WorkerHealthState;
  }): CancelablePromise<PaginatedResponse_WorkerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workers",
      query: {
        page: page,
        page_size: pageSize,
        role: role,
        status: status,
        cordoned: cordoned,
        health_state: healthState,
      },
    });
  }
  /**
   * @returns WorkerSummary Worker with runtime support and current load
   * @throws ApiError
   */
  public static getWorker({
    id,
  }: {
    /**
     * Worker ID
     */
    id: number;
  }): CancelablePromise<WorkerSummary> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workers/{id}",
      path: {
        id: id,
      },
      errors: {
        404: `Worker not found`,
      },
    });
  }
  /**
   * @returns WorkerSummary Worker cordoned
   * @throws ApiError
   */
  public static cordonWorker({
    id,
    requestBody,
  }: {
    /**
     * Worker ID
     */
    id: number;
    requestBody: CordonWorkerRequest;
  }): CancelablePromise<WorkerSummary> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workers/{id}/cordon",
      path: {
        id: id,
      },
      body: requestBody,
      mediaType: "application/json",
    });
  }
  /**
   * @returns WorkerSummary Worker uncordoned
   * @throws ApiError
   */
  public static uncordonWorker({
    id,
  }: {
    /**
     * Worker ID
     */
    id: number;
  }): CancelablePromise<WorkerSummary> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workers/{id}/uncordon",
      path: {
        id: id,
      },
    });
  }
}
