/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { i64 } from "./i64";
import type { LogAvailabilityState } from "./LogAvailabilityState";
export type LogAvailability = {
  artifact_version?: null | i64;
  finalize_by?: string | null;
  retry_after_ms?: number | null;
  revision?: number | null;
  state: LogAvailabilityState;
  stream?: null | i64;
};
