/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchEngineBinding } from "./LaunchEngineBinding";
import type { PinnedImage } from "./PinnedImage";
import type { WorkloadHostRole } from "./WorkloadHostRole";
export type HostExecutionProfile = {
  controller_origin: string;
  engine: LaunchEngineBinding;
  image: PinnedImage;
  max_concurrent: number;
  role: WorkloadHostRole;
};
