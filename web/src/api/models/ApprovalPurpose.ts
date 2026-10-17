/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { DeclaredInputApprovalTarget } from "./DeclaredInputApprovalTarget";
export type ApprovalPurpose =
  | {
      kind: "runtime";
    }
  | {
      kind: "declared_input";
      /**
       * Only currently visible retained targets. Empty when target rows were
       * collected or current component visibility denies disclosure. Purpose
       * never falls back to runtime merely because no target is visible.
       */
      targets: Array<DeclaredInputApprovalTarget>;
    };
