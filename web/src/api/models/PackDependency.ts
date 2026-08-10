/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Pack dependency information
 */
export type PackDependency = {
  /**
   * Whether dependency is already installed
   */
  already_installed: boolean;
  /**
   * Pack reference
   */
  pack_ref: string;
  /**
   * Pack that requires this dependency
   */
  required_by: string;
  /**
   * Version specification
   */
  version_spec: string;
};
