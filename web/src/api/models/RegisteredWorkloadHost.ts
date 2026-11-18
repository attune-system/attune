/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { HostExecutionProfile } from "./HostExecutionProfile";
import type { WorkloadSession } from "./WorkloadSession";
export type RegisteredWorkloadHost = {
  instance: string;
  profile: HostExecutionProfile;
  registration_generation: number;
  session: WorkloadSession;
  worker: number;
};
