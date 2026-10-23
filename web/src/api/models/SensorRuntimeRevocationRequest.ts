/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Explicit operational revocation, separate from disablement or human IAM edits.
 */
export type SensorRuntimeRevocationRequest = {
  /**
   * Revoke only this immutable revision. Omit to revoke every sensor generation.
   */
  metadata_revision?: number | null;
};
