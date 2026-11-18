/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EngineResourceEvidence } from "./EngineResourceEvidence";
export type ProviderCreateSettlement =
  | {
      kind: "never_submitted";
    }
  | {
      create_nonce: string;
      kind: "definitive_absence";
    }
  | {
      create_nonce: string;
      kind: "created";
      resource: EngineResourceEvidence;
    };
