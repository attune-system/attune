/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type ControlResponse =
  | {
      state: "waiting";
    }
  | {
      state: "settled";
    }
  | {
      state: "deliver";
      token: string;
    };
