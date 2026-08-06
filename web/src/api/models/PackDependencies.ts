/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Pack dependencies
 */
export type PackDependencies = {
  /**
   * Attune version requirement (semver)
   */
  attune_version?: string | null;
  /**
   * Node.js version requirement
   */
  nodejs_version?: string | null;
  /**
   * Pack dependencies (format: "ref@version")
   */
  packs?: Array<string>;
  /**
   * Python version requirement
   */
  python_version?: string | null;
};
