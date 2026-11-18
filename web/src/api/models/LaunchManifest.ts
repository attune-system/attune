/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { FrozenContextRef } from "./FrozenContextRef";
import type { LaunchPurpose } from "./LaunchPurpose";
import type { LaunchTarget } from "./LaunchTarget";
import type { PackAccess } from "./PackAccess";
import type { PinnedImage } from "./PinnedImage";
import type { PinnedLaunchInput } from "./PinnedLaunchInput";
import type { PrivateStage } from "./PrivateStage";
export type LaunchManifest = {
  context: FrozenContextRef;
  engine_id: number;
  engine_name_nonce: string;
  image: PinnedImage;
  inputs: Array<PinnedLaunchInput>;
  launch_id: number;
  /**
   * Initial admission deadline. Runtime renewals change the separate lease,
   * never this immutable manifest or its digest (notably for managed sensors).
   */
  lease_deadline: string;
  owner_identity_id: number;
  pack_access: PackAccess;
  purpose: LaunchPurpose;
  stage?: null | PrivateStage;
  target: LaunchTarget;
  timeout_ms?: number | null;
};
