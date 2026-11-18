/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Engine identity is immutable across controller restarts. Replacing a daemon
 * or changing its native platform requires a new engine ID; runtime cache
 * compatibility is never inferred from a multiarch index digest alone.
 */
export enum LaunchPlatform {
  LINUX_AMD64 = "linux/amd64",
  LINUX_ARM64 = "linux/arm64",
}
