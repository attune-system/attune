/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CustodyAbsence } from "../models/CustodyAbsence";
import type { CustodyOperationAck } from "../models/CustodyOperationAck";
import type { PrepareCustody } from "../models/PrepareCustody";
import type { RegisteredWorkloadHost } from "../models/RegisteredWorkloadHost";
import type { RegisterWorkloadHost } from "../models/RegisterWorkloadHost";
import type { RenewWorkloadSession } from "../models/RenewWorkloadSession";
import type { ReserveCustody } from "../models/ReserveCustody";
import type { WorkloadSession } from "../models/WorkloadSession";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class WorkloadControlService {
  /**
   * @returns CustodyAbsence
   * @throws ApiError
   */
  public static workloadHostCustodyCancelAbsent({
    requestBody,
  }: {
    requestBody: ReserveCustody;
  }): CancelablePromise<CustodyAbsence> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/custody/cancel-absent",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Custody unavailable`,
      },
    });
  }
  /**
   * @returns CustodyOperationAck
   * @throws ApiError
   */
  public static workloadHostCustodyPrepare({
    requestBody,
  }: {
    requestBody: PrepareCustody;
  }): CancelablePromise<CustodyOperationAck> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/custody/prepare",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Custody unavailable`,
      },
    });
  }
  /**
   * @returns CustodyOperationAck
   * @throws ApiError
   */
  public static workloadHostCustodyReserve({
    requestBody,
  }: {
    requestBody: ReserveCustody;
  }): CancelablePromise<CustodyOperationAck> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/custody/reserve",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Custody unavailable`,
      },
    });
  }
  /**
   * @returns RegisteredWorkloadHost
   * @throws ApiError
   */
  public static workloadHostRegister({
    requestBody,
  }: {
    requestBody: RegisterWorkloadHost;
  }): CancelablePromise<RegisteredWorkloadHost> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/register",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Host enrollment unavailable`,
      },
    });
  }
  /**
   * @returns WorkloadSession
   * @throws ApiError
   */
  public static workloadHostSession({
    requestBody,
  }: {
    requestBody: RenewWorkloadSession;
  }): CancelablePromise<WorkloadSession> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/session",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Host session unavailable`,
      },
    });
  }
}
