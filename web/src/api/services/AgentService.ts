/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { AgentBinaryInfo } from "../models/AgentBinaryInfo";
import type { CancelablePromise } from "../core/CancelablePromise";
import { OpenAPI } from "../core/OpenAPI";
import { request as __request } from "../core/request";
export class AgentService {
  /**
   * Download the agent binary
   * Returns the statically-linked attune-agent binary for the requested architecture.
   * The binary can be injected into any container to turn it into an Attune worker.
   * @returns any Agent binary
   * @throws ApiError
   */
  public static downloadAgentBinary({
    arch,
  }: {
    /**
     * Target architecture (x86_64, aarch64). Defaults to x86_64.
     */
    arch?: string | null;
  }): CancelablePromise<any> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/agent/binary",
      query: {
        arch: arch,
      },
      errors: {
        400: `Invalid architecture`,
        401: `Invalid or missing bootstrap token`,
        404: `Agent binary not found`,
        503: `Agent binary distribution not configured`,
      },
    });
  }
  /**
   * Get agent binary metadata
   * Returns information about available agent binaries, including
   * supported architectures and binary sizes.
   * @returns AgentBinaryInfo Agent binary info
   * @throws ApiError
   */
  public static agentInfo(): CancelablePromise<AgentBinaryInfo> {
    return __request(OpenAPI, {
      method: "GET",
      url: "/api/v1/agent/info",
      errors: {
        503: `Agent binary distribution not configured`,
      },
    });
  }
}
