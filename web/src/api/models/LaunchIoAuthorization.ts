/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchHandle } from "./LaunchHandle";
import type { LaunchIoChannel } from "./LaunchIoChannel";
export type LaunchIoAuthorization = {
  channel: LaunchIoChannel;
  expires_at: string;
  handle: LaunchHandle;
  max_bytes: number;
};
