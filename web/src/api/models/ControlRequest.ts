/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Exact persisted custody binding; no payload or caller-selected authority.
 */
export type ControlRequest = {
  attempt: string;
  custody: string;
  execution: number;
  source_instance: string;
  stream: number;
  version: number;
};
