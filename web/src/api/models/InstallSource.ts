/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Installation source for a pack
 */
export type InstallSource =
  | {
      /**
       * Checksum in format "algorithm:hash"
       */
      checksum: string;
      /**
       * Git ref (tag, branch, commit)
       */
      ref: string;
      type: "git";
      /**
       * Git repository URL
       */
      url: string;
    }
  | {
      /**
       * Checksum in format "algorithm:hash"
       */
      checksum: string;
      type: "archive";
      /**
       * Archive URL
       */
      url: string;
    };
