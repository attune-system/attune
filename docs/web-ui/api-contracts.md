# Web API contract checks

Run these commands from `web/` after changing an API DTO or form request:

```bash
npm run generate:api
npm run check:api
npm run test:api-generation
npm run typecheck
npm run typecheck:contracts
npm test
```

`generate:api` exports OpenAPI from the current Rust sources. No running API or database is needed. `check:api` regenerates into a temporary directory and fails if the checked-in client differs, without rewriting it. `--input` accepts an existing spec; `--output` directs generation or comparison to another directory. `--models` scopes model generation but does not update service signatures or the index. Use full regeneration before running the full freshness check.

The published OpenAPI document remains 3.1. The generator adapts nullable type arrays to the code generator's `nullable` representation so array element types survive. Models with inline patch discriminants use literal unions so `set` requires a value and `clear` remains a separate variant. Other enum exports remain enums. The unconstrained `serde_json::Value` schema generates a recursive JSON value type, including null and scalars.

Dynamic values from schema editors and key forms pass through `web/src/lib/json-value.ts` at submission. It constructs typed JSON without request assertions, rejects unsupported values and cycles, and omits undefined object fields without including rejected contents in error messages.

`contracts/api.types.ts` has positive assignments and negative `@ts-expect-error` assertions. A weakened generated type fails the negative assertions; an overly restrictive type fails the positive assignments. Optional JSON update fields must also be marked `required = false` in Rust schema annotations when `value_type` overrides the inferred type.

Form component tests compare their serialized request bodies with `web/src/test/fixtures/form-requests.json`. `crates/api/tests/web_form_contract_tests.rs` passes those same fixtures through Axum's JSON extractor and the real request DTO validators. It also checks 422 rejection of identity fields and malformed patches, plus OpenAPI partial-update contracts. Run it from the repository root:

```bash
ATTUNE_TEST_RUN_ID=web-contract-local cargo test -p attune-api \
  --test web_form_contract_tests -- --test-threads=4
```

CI runs these checks on web changes and API/common Rust changes. Generated TypeScript is not runtime validation, and TypeScript allows extra properties through variables and spreads. Build explicit update bodies, keep create-only identity fields out of shared payloads, and do not cast request bodies to force them through checking. For nullable patches, `null` or omission preserves the current value; an explicit `{"op":"clear"}` clears it.
