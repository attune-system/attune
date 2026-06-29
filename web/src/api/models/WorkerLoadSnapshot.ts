/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type WorkerLoadSnapshot = {
  active_rules?: number | null;
  canceling: number;
  max_concurrent_executions?: number | null;
  max_concurrent_sensors?: number | null;
  queue_depth?: number | null;
  requested: number;
  running: number;
  scheduled: number;
  scheduling: number;
  sensor_processes_monitored?: number | null;
  sensor_processes_running?: number | null;
  total_active: number;
  utilization_percent?: number | null;
};
