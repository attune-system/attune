/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Python environment details
 */
export type PythonEnvironment = {
  /**
   * Number of packages installed
   */
  package_count: number;
  /**
   * Python version used
   */
  python_version: string;
  /**
   * Whether requirements were installed
   */
  requirements_installed: boolean;
  /**
   * Path to virtualenv
   */
  virtualenv_path: string;
};
