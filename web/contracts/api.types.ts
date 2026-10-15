import type {
  CreateRuntimeRequest,
  ArtifactJsonPatch,
  ArtifactStringPatch,
  LogRetentionLimitPatch,
  LogRetentionPolicyPatch,
  NullableJsonPatch,
  NullableStringPatch,
  PackDescriptionPatch,
  RuntimeVersionConstraintPatch,
  TimeoutSecondsPatch,
  UpdateRuleRequest,
  UpdateRuntimeRequest,
  UpdateTriggerRequest,
} from "../src/api";

type Patches = {
  artifactJson: ArtifactJsonPatch;
  artifactString: ArtifactStringPatch;
  retentionLimit: LogRetentionLimitPatch;
  retentionPolicy: LogRetentionPolicyPatch;
  json: NullableJsonPatch;
  string: NullableStringPatch;
  packDescription: PackDescriptionPatch;
  runtimeVersion: RuntimeVersionConstraintPatch;
  timeout: TimeoutSecondsPatch;
};

export const clears: Patches = {
  artifactJson: { op: "clear" },
  artifactString: { op: "clear" },
  retentionLimit: { op: "clear" },
  retentionPolicy: { op: "clear" },
  json: { op: "clear" },
  string: { op: "clear" },
  packDescription: { op: "clear" },
  runtimeVersion: { op: "clear" },
  timeout: { op: "clear" },
};

type Assert<T extends true> = T;
export type SetsRequireValues = Assert<
  {
    [K in keyof Patches]: Extract<Patches[K], { op: "set" }> extends {
      value: unknown;
    }
      ? true
      : false;
  }[keyof Patches]
>;

export const partialRule = { label: "Updated" } satisfies UpdateRuleRequest;
export const partialRuntime = {
  name: "Updated",
} satisfies UpdateRuntimeRequest;
export const jsonScalar = {
  op: "set",
  value: null,
} satisfies NullableJsonPatch;

export const scalarInstallation = {
  ref: "demo.scalar",
  name: "Scalar installation",
  installation: "system",
} satisfies CreateRuntimeRequest;

// @ts-expect-error set operations require a value
export const missingValue: NullableStringPatch = { op: "set" };

export const invalidPermissionRefs: UpdateRuleRequest["permission_set_refs"] = [
  // @ts-expect-error permission refs are strings, not arbitrary JSON
  123,
];

export const invalidRule = {
  // @ts-expect-error pack ownership is not editable
  pack_ref: "demo",
} satisfies UpdateRuleRequest;

export const invalidTrigger = {
  // @ts-expect-error identity belongs in the URL, not the update body
  ref: "demo.trigger",
} satisfies UpdateTriggerRequest;
