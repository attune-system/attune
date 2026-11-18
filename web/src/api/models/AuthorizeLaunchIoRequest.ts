/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchIoChannel } from "./LaunchIoChannel";
import type { LaunchRequest } from "./LaunchRequest";
/**
 * Controller authenticates with its CP credential and current lease, forwarding
 * the manager's exact channel credential in LAUNCH_IO_HEADER before byte effects.
 */
export type AuthorizeLaunchIoRequest = {
  bytes: number;
  channel: LaunchIoChannel;
  offset: number;
  request: LaunchRequest;
};
