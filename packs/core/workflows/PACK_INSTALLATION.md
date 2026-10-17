# Pack installation workflow

Core 1.0.4 ships `core.install_packs` as a canonical v1 workflow action:

- `actions/install_packs.yaml` owns the public parameters, outputs and ref.
- `actions/workflows/install_packs.workflow.yaml` owns the graph.
- No standalone workflow YAML, dual reader or legacy graph is required.

The workflow accepts explicit Git or HTTP archive URLs. Registry refs must use
`attune pack install` or `POST /api/v1/packs/install`, where registry identity,
checksum provenance and ordinary admission remain bound together. Direct-source
installation still requires the deployment's existing opt-in and source-host
policy. The workflow does not change either policy.

## Stages and actual behavior

1. `core.download_packs` calls `/api/v1/packs/download`. The API chooses staging
   paths and returns downloaded and failed sources. The action also returns a
   typed `pack_paths` array for subsequent tasks. Partial downloads continue with
   the successful paths; zero successful downloads fail the workflow.
2. Unless `skip_dependencies` is true, `core.get_pack_dependencies` checks those
   paths through `/api/v1/packs/dependencies`. Missing dependencies or manifest
   errors fail the workflow. Install missing dependencies separately through
   ordinary admission. The workflow never recursively interprets dependency names
   as unverified URLs. `installed_dependencies` therefore remains an empty array.
3. Unless `skip_env_build` is true, `core.build_pack_envs` calls `/api/v1/packs/build-envs`.
   This endpoint reports API-host interpreter detection, not completed environment
   builds or worker readiness. A detection failure stops the workflow unless
   `force` is true. Workers own actual environment setup for tests and executions.
4. `core.register_packs` calls `/api/v1/packs/register-batch`, passing `skip_tests`
   and `force` independently. Ordinary API registration owns candidate tests,
   immutable releases, configuration, policy admission and activation. There is
   no separate `core.run_pack_tests` action. `force` permits replacement, not an
   approval exemption, test skip or incompatible-rollout acknowledgement.

The registration API returns per-pack results. A batch is not one all-or-none
transaction. Zero successful registrations fail the workflow. Partial registration
returns successful and failed packs with counts; it does not undo committed packs.
Results may include pending candidate `install_id` and `install_status` fields.
An accepted registration request is not evidence that those tests finished or
that the new release activated. Inspect the returned install status before use.
No automatic retry or pointer rewind occurs after an uncertain HTTP outcome.

The failure terminal uses the existing `core.noop` action with `exit_code: 1`.
It does not turn a handled error into a successful parent. The historical cleanup
tasks never deleted anything; they are removed. The API owns staging lifecycle.
The workflow cannot safely delete API-host paths from an arbitrary worker.

## Inputs and outputs

The existing input names and defaults remain: required nonempty `packs` array,
optional `ref_spec`, four false flags `skip_dependencies`, `skip_tests`,
`skip_env_build`, `force`, `packs_base_dir=/opt/attune/packs`,
`api_url=http://localhost:8080` and `timeout=1800`, bounded to 300..7200 seconds.
An absent `ref_spec` now defaults to an empty string, which the wrapper omits
from the API request. Server configuration remains authoritative over storage.

`timeout` divides the task-time budget across four API stages. Queueing and
scheduler overhead are additional; it is not a global wall-clock deadline.
`summary.duration_seconds` sums completed API request durations, not queue time.
Outputs retain `registered_packs`, `failed_packs`, `installed_dependencies` and
`summary`. Failure details retain the API's per-stage shapes.

The optional secret `api_token` is an existing bearer access token for ordinary
API authorization. It is passed only as secret child input and an HTTP header,
never in a URL, command line, published variable or output. Without that input,
the wrapper uses `ATTUNE_API_TOKEN` only if an authorized execution supplied it,
and only sends it to that execution's `ATTUNE_API_URL`, never an overridden host.
The secret has no schema default, not even an empty string or null. Native
`'api_token' in parameters` guards select one of two input shapes for each API
stage. The absent-token shape omits both `api_token` and `api_url` entirely, and
child schemas do not inject an API URL default ahead of the context URL. The
provided-token shape retains direct `parameters.api_token` source evidence and
uses the public `api_url` input/default. The four pairs are mutually exclusive
calls to the same shipped actions, not extra stages or parallel requests. YAML
aliases share their canonical next transitions. No unsupported safe-get function,
missing-property lookup or manufactured empty secret is used.
The pack adds no grants, identities, permission assignments or approval bypass.
In particular, `standard` does not grant pack installation. Select an existing
properly admitted authority or supply a suitably scoped access token. Raw
integration credentials must first use the existing token-login exchange.

The four supporting actions keep their refs and `.sh` entrypoint names, but now
use canonical stdin JSON and declare Python 3.11+ as a worker requirement. Their
shared stdlib transport preserves nested/quoted arrays and booleans, unwraps the
API response with a real JSON parser, refuses redirects/proxies, bounds response
size and fails on malformed result fields. No dotenv/JSON dual reader remains.

## Validation and rollout

Run the offline source tests without launching services or pack actions:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest \
  scripts/test_core_pack_workflow.py scripts/test_core_bootstrap_composition.py
```

These check the shipped graph, real action bindings, schemas/defaults, secret
input paths and mocked HTTP transport. They do not execute the Rust parser,
planner, scheduler or API. The central PostgreSQL 18 runner owns admission tests;
full worker/API lifecycle acceptance remains separate. No readiness gate is
enabled by this pack migration.

`crates/executor/tests/core_pack_workflow_contract_tests.rs` also defines pure
regressions through the actual flat compiler, workflow parser and sensitivity
renderer. They compile the shipped action schemas, reject a reintroduced empty
or null secret default, evaluate actual presence guards, and check both omitted
child fields and provided secret-origin evidence. These Rust tests still require
execution in the central validation lane; offline Python checks are not a
substitute for them or for persisted input-receipt authorization tests.

Core's version changes from 1.0.3 to 1.0.4 because the archive and public component
set changed. Publish compatible images containing these pack bytes and select
the matching core version and archive digest. Do not reuse an old digest or
silently rewrite an immutable 1.0.3 release.
