/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Value } from "./Value";
/**
 * Explicit patch operation for nullable JSON fields.
 */
export type NullableJsonPatch =
  | {
      op: NullableJsonPatch.op;
      value: Value;
    }
  | {
      op: NullableJsonPatch.op;
    };
export namespace NullableJsonPatch {
  export enum op {
    SET = "set",
  }
}
