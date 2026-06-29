/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { RetentionPolicyType } from "./RetentionPolicyType";
export type LogRetentionPolicyPatch =
  | {
      op: LogRetentionPolicyPatch.op;
      value: RetentionPolicyType;
    }
  | {
      op: LogRetentionPolicyPatch.op;
    };
export namespace LogRetentionPolicyPatch {
  export enum op {
    SET = "set",
  }
}
