/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type CustodyAbsence = {
  /**
   * True only after an acknowledged permanent tombstone/history fence.
   * False means an existing intent must be reconciled, not deleted locally.
   */
  absent: boolean;
  custody: string;
};
