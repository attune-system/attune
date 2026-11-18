/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type LaunchTarget =
  | {
      attempt: string;
      execution: number;
      kind: "execution";
      worker: number;
      worker_instance: string;
    }
  | {
      generation: number;
      kind: "sensor";
      sensor: number;
      worker: number;
      worker_instance: string;
      workload: number;
    }
  | {
      admission: number;
      attempt: string;
      kind: "setup";
    }
  | {
      admission: number;
      attempt: string;
      kind: "runtime_probe";
    }
  | {
      attempt: string;
      install: number;
      kind: "candidate_test";
      test_job: number;
    };
