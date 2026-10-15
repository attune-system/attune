/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Explicit patch operation for a nullable action default timeout (seconds).
 */
export type TimeoutSecondsPatch =
  | {
      op: "set";
      value: number;
    }
  | {
      op: "clear";
    };
