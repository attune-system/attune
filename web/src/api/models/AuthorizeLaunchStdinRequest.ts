/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchHandle } from "./LaunchHandle";
import type { LaunchStdinPayload } from "./LaunchStdinPayload";
/**
 * The controller holds these exact immutable bytes until the acknowledged
 * authorization, then writes them once without re-reading a mutable file or
 * changing chunks. Lost/uncertain ACK is fail-closed: no retry may create a
 * second effect. LAUNCH_LEASE_HEADER and LAUNCH_IO_HEADER carry credentials.
 */
export type AuthorizeLaunchStdinRequest = {
  handle: LaunchHandle;
  payload: LaunchStdinPayload;
};
