/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Controller restart discovery is engine-scoped by authentication. Complete a
 * bounded cursor round, then begin a new round; unfinished rows are not consumed.
 */
export type ReconcileLaunchesRequest = {
  after?: number | null;
  limit: number;
};
