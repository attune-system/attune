/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { MaintenanceScheduleStatus } from "./MaintenanceScheduleStatus";
import type { PartitionStatus } from "./PartitionStatus";
import type { SummaryStatus } from "./SummaryStatus";
export type NativeMaintenanceStatus = {
  /**
   * Persisted native-maintenance switch, independent of row retention.
   */
  enabled: boolean;
  /**
   * UTC time at which the API began collecting these observations.
   */
  observed_at: string;
  partitions: Array<PartitionStatus>;
  schedule: Array<MaintenanceScheduleStatus>;
  /**
   * Coverage extrema do not imply continuous coverage. Dirty hours use raw reads.
   */
  summaries: Array<SummaryStatus>;
};
