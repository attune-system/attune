/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Explicit patch operation for nullable string fields.
 */
export type NullableStringPatch =
  | {
      op: NullableStringPatch.op;
      value: string;
    }
  | {
      op: NullableStringPatch.op;
    };
export namespace NullableStringPatch {
  export enum op {
    SET = "set",
  }
}
