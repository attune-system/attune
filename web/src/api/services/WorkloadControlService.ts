/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AuthorizeLaunchIoRequest } from "../models/AuthorizeLaunchIoRequest";
import type { AuthorizeLaunchStdinRequest } from "../models/AuthorizeLaunchStdinRequest";
import type { CanonicalLaunchZip } from "../models/CanonicalLaunchZip";
import type { ClaimedLaunch } from "../models/ClaimedLaunch";
import type { CustodyAbsence } from "../models/CustodyAbsence";
import type { CustodyOperationAck } from "../models/CustodyOperationAck";
import type { LaunchCreateIntent } from "../models/LaunchCreateIntent";
import type { LaunchEngineBinding } from "../models/LaunchEngineBinding";
import type { LaunchIoAuthorization } from "../models/LaunchIoAuthorization";
import type { LaunchIoGrant } from "../models/LaunchIoGrant";
import type { LaunchIoGrantRequest } from "../models/LaunchIoGrantRequest";
import type { LaunchLease } from "../models/LaunchLease";
import type { LaunchLeaseRequest } from "../models/LaunchLeaseRequest";
import type { LaunchManifest } from "../models/LaunchManifest";
import type { LaunchOutputGrant } from "../models/LaunchOutputGrant";
import type { LaunchRequest } from "../models/LaunchRequest";
import type { LaunchStageResult } from "../models/LaunchStageResult";
import type { LaunchStartIntent } from "../models/LaunchStartIntent";
import type { LaunchStatus } from "../models/LaunchStatus";
import type { NextActionRequest } from "../models/NextActionRequest";
import type { NextActionResponse } from "../models/NextActionResponse";
import type { ObserveLaunchRequest } from "../models/ObserveLaunchRequest";
import type { PrepareCustody } from "../models/PrepareCustody";
import type { ReconcileLaunchesPage } from "../models/ReconcileLaunchesPage";
import type { ReconcileLaunchesRequest } from "../models/ReconcileLaunchesRequest";
import type { RegisteredWorkloadHost } from "../models/RegisteredWorkloadHost";
import type { RegisterWorkloadHost } from "../models/RegisterWorkloadHost";
import type { RenewWorkloadSession } from "../models/RenewWorkloadSession";
import type { ReserveCustody } from "../models/ReserveCustody";
import type { ReserveLaunchOutput } from "../models/ReserveLaunchOutput";
import type { Sha256Digest } from "../models/Sha256Digest";
import type { StopLaunchAcknowledgement } from "../models/StopLaunchAcknowledgement";
import type { StopLaunchRequest } from "../models/StopLaunchRequest";
import type { WorkloadSession } from "../models/WorkloadSession";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class WorkloadControlService {
  /**
   * @returns LaunchIoGrant
   * @throws ApiError
   */
  public static workloadHostIoGrant({
    requestBody,
  }: {
    requestBody: LaunchIoGrantRequest;
  }): CancelablePromise<LaunchIoGrant> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/actions/io-grants",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Channel unavailable`,
      },
    });
  }
  /**
   * @returns NextActionResponse
   * @throws ApiError
   */
  public static workloadHostNextAction({
    requestBody,
  }: {
    requestBody: NextActionRequest;
  }): CancelablePromise<NextActionResponse> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-hosts/actions/next",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Action unavailable`,
      },
    });
  }
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
  /**
   * @returns LaunchEngineBinding
   * @throws ApiError
   */
  public static workloadLaunchEngineBinding(): CancelablePromise<LaunchEngineBinding> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/workload-launches/engine",
      errors: {
        403: `Engine unavailable`,
      },
    });
  }
  /**
   * @returns ReconcileLaunchesPage
   * @throws ApiError
   */
  public static workloadLaunchReconcile({
    requestBody,
  }: {
    requestBody: ReconcileLaunchesRequest;
  }): CancelablePromise<ReconcileLaunchesPage> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/reconcile",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns WorkloadSession
   * @throws ApiError
   */
  public static workloadLaunchSession({
    requestBody,
  }: {
    requestBody: RenewWorkloadSession;
  }): CancelablePromise<WorkloadSession> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/session",
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Controller session unavailable`,
      },
    });
  }
  /**
   * @returns LaunchIoAuthorization
   * @throws ApiError
   */
  public static workloadLaunchAuthorizeIo({
    id,
    xAttuneLaunchLease,
    xAttuneLaunchIo,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchIo: string;
    requestBody: AuthorizeLaunchIoRequest;
  }): CancelablePromise<LaunchIoAuthorization> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/authorize-io",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
        "x-attune-launch-io": xAttuneLaunchIo,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchIoAuthorization
   * @throws ApiError
   */
  public static workloadLaunchAuthorizeStdin({
    id,
    xAttuneLaunchLease,
    xAttuneLaunchIo,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    /**
     * Exact stdin grant
     */
    xAttuneLaunchIo: string;
    requestBody: AuthorizeLaunchStdinRequest;
  }): CancelablePromise<LaunchIoAuthorization> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/authorize-stdin",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
        "x-attune-launch-io": xAttuneLaunchIo,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Private input unavailable`,
      },
    });
  }
  /**
   * @returns ClaimedLaunch
   * @throws ApiError
   */
  public static workloadLaunchClaim({
    id,
    xAttuneLaunchDispatch,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchDispatch: string;
    requestBody: LaunchRequest;
  }): CancelablePromise<ClaimedLaunch> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/claim",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-dispatch": xAttuneLaunchDispatch,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchCreateIntent
   * @throws ApiError
   */
  public static workloadLaunchCreateIntent({
    id,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: LaunchLeaseRequest;
  }): CancelablePromise<LaunchCreateIntent> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/create-intent",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns any Exact pinned ZIP body
   * @throws ApiError
   */
  public static workloadLaunchInputBody({
    id,
    pinId,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    pinId: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: LaunchLeaseRequest;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/inputs/{pin_id}/body",
      path: {
        id: id,
        pin_id: pinId,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchIoGrant
   * @throws ApiError
   */
  public static workloadLaunchIoGrants({
    id,
    xAttuneLaunchDispatch,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchDispatch: string;
    requestBody: LaunchIoGrantRequest;
  }): CancelablePromise<LaunchIoGrant> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/io-grants",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-dispatch": xAttuneLaunchDispatch,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStatus
   * @throws ApiError
   */
  public static workloadLaunchObserve({
    id,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: ObserveLaunchRequest;
  }): CancelablePromise<LaunchStatus> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/observe",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchOutputGrant
   * @throws ApiError
   */
  public static workloadLaunchReserveOutput({
    id,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: ReserveLaunchOutput;
  }): CancelablePromise<LaunchOutputGrant> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/outputs",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStageResult
   * @throws ApiError
   */
  public static workloadLaunchOutputBody({
    id,
    outputId,
    admittedDigest,
    xAttuneLaunchLease,
    xAttuneLaunchOutput,
    requestBody,
  }: {
    id: number;
    outputId: number;
    admittedDigest: Sha256Digest;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchOutput: string;
    requestBody: CanonicalLaunchZip;
  }): CancelablePromise<LaunchStageResult> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/outputs/{output_id}/body",
      path: {
        id: id,
        output_id: outputId,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
        "x-attune-launch-output": xAttuneLaunchOutput,
      },
      query: {
        admitted_digest: admittedDigest,
      },
      body: requestBody,
      mediaType: "application/zip",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchLease
   * @throws ApiError
   */
  public static workloadLaunchRenew({
    id,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: LaunchLeaseRequest;
  }): CancelablePromise<LaunchLease> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/renew",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchManifest
   * @throws ApiError
   */
  public static workloadLaunchResolve({
    id,
    xAttuneLaunchDispatch,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchDispatch: string;
    requestBody: LaunchRequest;
  }): CancelablePromise<LaunchManifest> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/resolve",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-dispatch": xAttuneLaunchDispatch,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStageResult
   * @throws ApiError
   */
  public static workloadLaunchStageResult({
    id,
    xAttuneLaunchDispatch,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchDispatch: string;
    requestBody: LaunchRequest;
  }): CancelablePromise<LaunchStageResult> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/stage-result",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-dispatch": xAttuneLaunchDispatch,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStartIntent
   * @throws ApiError
   */
  public static workloadLaunchStart({
    id,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: LaunchLeaseRequest;
  }): CancelablePromise<LaunchStartIntent> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/start",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStatus
   * @throws ApiError
   */
  public static workloadLaunchStatus({
    id,
    xAttuneLaunchDispatch,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchDispatch: string;
    requestBody: LaunchRequest;
  }): CancelablePromise<LaunchStatus> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/status",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-dispatch": xAttuneLaunchDispatch,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStatus
   * @throws ApiError
   */
  public static workloadLaunchStop({
    id,
    xAttuneLaunchDispatch,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchDispatch: string;
    requestBody: StopLaunchRequest;
  }): CancelablePromise<LaunchStatus> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/stop",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-dispatch": xAttuneLaunchDispatch,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
  /**
   * @returns LaunchStatus
   * @throws ApiError
   */
  public static workloadLaunchStopAck({
    id,
    xAttuneLaunchLease,
    requestBody,
  }: {
    id: number;
    /**
     * Exact target-scoped grant
     */
    xAttuneLaunchLease: string;
    requestBody: StopLaunchAcknowledgement;
  }): CancelablePromise<LaunchStatus> {
    return __request(OpenAPI, {
      method: "POST",
      url: "/api/v1/workload-launches/{id}/stop-ack",
      path: {
        id: id,
      },
      headers: {
        "x-attune-launch-lease": xAttuneLaunchLease,
      },
      body: requestBody,
      mediaType: "application/json",
      errors: {
        403: `Launch unavailable`,
      },
    });
  }
}
