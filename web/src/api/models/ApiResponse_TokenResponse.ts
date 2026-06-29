/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { UserInfo } from "./UserInfo";
/**
 * Standard API response wrapper
 */
export type ApiResponse_TokenResponse = {
  /**
   * Token response
   */
  data: {
    /**
     * Access token (JWT)
     */
    access_token: string;
    /**
     * Access token expiration in seconds
     */
    expires_in: number;
    /**
     * Refresh token
     */
    refresh_token: string;
    /**
     * Token type (always "Bearer")
     */
    token_type: string;
    user?: null | UserInfo;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
