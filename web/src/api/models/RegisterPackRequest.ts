/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Request DTO for registering a pack from local filesystem
 */
export type RegisterPackRequest = {
  /**
   * Force registration even if tests fail
   */
  force?: boolean;
  /**
   * Local filesystem path to the pack directory
   */
  path: string;
  /**
   * Skip running pack tests during registration
   */
  skip_tests?: boolean;
};
