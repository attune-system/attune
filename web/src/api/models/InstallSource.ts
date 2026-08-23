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
      type: InstallSource.type;
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
      type: InstallSource.type;
      /**
       * Archive URL
       */
      url: string;
    };
export namespace InstallSource {
  export enum type {
    GIT = "git",
  }
}
