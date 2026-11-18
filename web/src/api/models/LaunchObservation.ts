/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { EngineResourceEvidence } from "./EngineResourceEvidence";
export type LaunchObservation =
  | {
      create_nonce: string;
      kind: "create_unknown";
    }
  | {
      create_nonce: string;
      kind: "created";
      resource: EngineResourceEvidence;
    }
  | {
      kind: "start_unknown";
      resource: EngineResourceEvidence;
      start_nonce: string;
    }
  | {
      kind: "started";
      resource: EngineResourceEvidence;
      start_nonce: string;
    }
  | {
      exit_code: number;
      kind: "exited";
      oom_killed: boolean;
      resource: EngineResourceEvidence;
      start_nonce: string;
    };
