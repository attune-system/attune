/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Explicit patch operation for a nullable runtime version constraint.
 */
export type RuntimeVersionConstraintPatch =
  | {
      op: "set";
      value: string;
    }
  | {
      op: "clear";
    };
