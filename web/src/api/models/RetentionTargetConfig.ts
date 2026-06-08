/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Runtime database row retention settings.
 *
 * A target with `max_age_seconds: None` keeps rows forever (purging disabled).
 * A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
 */
export type RetentionTargetConfig = {
    /**
     * Maximum row age in seconds. `None` means keep forever (no purging).
     */
    max_age_seconds?: number | null;
};

