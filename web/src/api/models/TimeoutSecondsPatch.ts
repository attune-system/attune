/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Explicit patch operation for a nullable action default timeout (seconds).
 */
export type TimeoutSecondsPatch =
  | {
      op: TimeoutSecondsPatch.op;
      value: number;
    }
  | {
      op: TimeoutSecondsPatch.op;
    };
export namespace TimeoutSecondsPatch {
  export enum op {
    SET = "set",
  }
}
