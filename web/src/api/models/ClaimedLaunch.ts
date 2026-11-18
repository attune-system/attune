/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { LaunchLease } from "./LaunchLease";
import type { LaunchManifest } from "./LaunchManifest";
import type { LaunchSpec } from "./LaunchSpec";
import type { LaunchStatus } from "./LaunchStatus";
export type ClaimedLaunch =
  | {
      lease: LaunchLease;
      mode: "active";
      spec: LaunchSpec;
    }
  | {
      lease: LaunchLease;
      manifest: LaunchManifest;
      mode: "cleanup";
      /**
       * Recorded original evidence remains usable after source/image revocation.
       */
      status: LaunchStatus;
    }
  | {
      lease: LaunchLease;
      manifest: LaunchManifest;
      mode: "capture";
      status: LaunchStatus;
    }
  | {
      lease: LaunchLease;
      manifest: LaunchManifest;
      mode: "publication";
      status: LaunchStatus;
    }
  | {
      mode: "stopped";
      status: LaunchStatus;
    };
