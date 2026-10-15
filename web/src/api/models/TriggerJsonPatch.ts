/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { TriggerClearOperation } from "./TriggerClearOperation";
import type { TriggerSetOperation } from "./TriggerSetOperation";
import type { Value } from "./Value";
export type TriggerJsonPatch =
  | {
      op: TriggerSetOperation;
      value: Value;
    }
  | {
      op: TriggerClearOperation;
    };
