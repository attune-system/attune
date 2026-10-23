# Pack and sensor configuration overrides

Pack and sensor configuration use the same field-level model. A release supplies
defaults. Operators supply persistent overrides. An omitted override field follows
the current release default, including schema defaults. An object or list override
replaces that complete top-level schema field. It does not recursively merge with
the default. Delivery does not reinsert defaults inside an overridden object.

## API contract

`PUT /api/v1/packs/{ref}` and `PUT /api/v1/sensors/{ref}` accept:

```json
{
  "config": {"endpoint": "https://example.test", "optional_value": null},
  "config_reset_fields": ["retry_policy"]
}
```

- `config` is an object of fields to override. Omitted fields retain their existing
  override state. `{}` changes no values.
- A field explicitly set to `null` is an override. Its schema must accept null.
- `config_reset_fields` contains literal top-level field names, not JSON Pointers.
  Reset removes an override and selects the current default, or removes the value
  when no default exists. Required-field validation still applies.
- Setting and resetting the same field is rejected. Unknown reset fields and new
  fields outside a nonempty schema/default/existing field set are rejected.
  Schema-free configuration remains open. Existing historical extra fields survive.
- Unchanged redacted fields can round-trip without disclosing their values.
  Editing part of a secret object requires replacing that complete field.

Defaults and effective values have immutable encrypted revisions. Current layer
state contains only a defaults revision ID and overridden field names. It never
contains a second plaintext secret copy. Historical effective values remain bound
to their original contract. Missing layer history is not evidence of a default;
existing bound fields become overrides until explicitly reset.

Schema migration authorization compares semantic contracts. Equivalent preserved
bindings do not require unrelated Key grants or decrypt authority. Incompatible
preserved values reject the transaction with value-free errors.

Sensor `input_permission_set_refs`, `cache_permission_set_refs`, and
`signing_permission_set_refs` are admitted executable metadata. Ordinary overrides
cannot change or reset them. Configuration template changes and managed sensor
code/schema changes also require deployment readmission. Harmless labels do not.

## Trigger contract rollout

`POST /api/v1/triggers/{ref}/rollout-preflight` accepts the same schema patches as
trigger PUT. It returns `trigger_id`, `execution_ids`, and `workload_ids` without
configuration or payload values. It requires trigger update authority.

Trigger PUT uses `acknowledge_incompatible_rollout: true` to explicitly accept a
changed contract with live admitted emitters. The default is false. Ordinary
deployment confirmation, force, or test bypass is not this acknowledgement.

The safe procedure is to stop new admission, drain affected executions and sensor
workloads, then activate. Preflight is advisory; activation repeats it under the
exclusive schema graph lock and keeps that lock through commit. New execution
admissions and sensor generation selection use shared graph admission. A change
committed first becomes the current event contract. An earlier admitted execution
keeps its authority, but its old payload may fail the deliberately changed target
contract. Generation-specific event routing is not provided.

Any changed declared contract is conservatively considered incompatible. This is
not a general JSON Schema compatibility prover. Named-type graph rollout gates
remain in force. Revoked/expired external approvals can conservatively over-report
an affected emitter; they cannot hide one.

## Deployment integration

Before changing existing projections that lack a configuration revision, bind their
old values to preserve unknown history. Activation passes authored candidate config
to `bind_release_pack` and `bind_release_sensor`, not an already-preserved mutable
projection. It runs preserved-schema migration authorization before binding and
commits defaults, overrides, active metadata, and policy together. Pack activation
must call trigger rollout preflight before overwriting each old trigger contract.
