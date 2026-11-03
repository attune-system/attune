/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { MaintenanceJob } from "./MaintenanceJob";
export type MaintenanceScheduleStatus = {
  deferred_since?: string | null;
  job: MaintenanceJob;
  last_success?: string | null;
  next_due: string;
};
