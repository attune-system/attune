/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchIoGrant } from "./LaunchIoGrant";
import type { ReservedLaunch } from "./ReservedLaunch";
/**
 * A dedicated response carrying selected input to the non-pack custodian.
 * Values are excluded from Debug, audit events, and public execution DTOs.
 */
export type AssignedAction = {
  controller_origin: string;
  execution: number;
  launch: ReservedLaunch;
  max_stderr_bytes: number;
  max_stdout_bytes: number;
  stderr_grant: LaunchIoGrant;
  stdin_grant: LaunchIoGrant;
  stdin_json: string;
  stdout_grant: LaunchIoGrant;
};
