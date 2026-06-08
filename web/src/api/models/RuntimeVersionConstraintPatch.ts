/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Explicit patch operation for a nullable runtime version constraint.
 */
export type RuntimeVersionConstraintPatch = ({
    op: RuntimeVersionConstraintPatch.op;
    value: string;
} | {
    op: RuntimeVersionConstraintPatch.op;
});
export namespace RuntimeVersionConstraintPatch {
    export enum op {
        SET = 'set',
    }
}

