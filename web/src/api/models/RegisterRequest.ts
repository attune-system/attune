/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Register request
 */
export type RegisterRequest = {
  /**
   * Display name (optional)
   */
  display_name?: string | null;
  /**
   * Identity login (username)
   */
  login: string;
  /**
   * Password
   */
  password: string;
};
