/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EngineResourceEvidence } from "./EngineResourceEvidence";
import type { LaunchCaptureState } from "./LaunchCaptureState";
import type { LaunchExitOutcome } from "./LaunchExitOutcome";
import type { LaunchPublicationState } from "./LaunchPublicationState";
import type { LaunchRequest } from "./LaunchRequest";
import type { LaunchRuntimeState } from "./LaunchRuntimeState";
import type { LaunchState } from "./LaunchState";
import type { LaunchStopReason } from "./LaunchStopReason";
export type LaunchStatus = {
  capture: LaunchCaptureState;
  capture_deadline?: string | null;
  exit_outcome?: null | LaunchExitOutcome;
  publication: LaunchPublicationState;
  request: LaunchRequest;
  resource?: null | EngineResourceEvidence;
  runtime_state: LaunchRuntimeState;
  state: LaunchState;
  stop_reason?: null | LaunchStopReason;
};
