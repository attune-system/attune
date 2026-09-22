# Provider-neutral inquiry callback ingress

## Status

Proposed specification for public HTTP callback providers. Managed sensors already have a metadata-selected trusted-transport ingress for localhost and private deployments. No running Attune deployment depends on the external provider listener or `POST /api/v1/inquiry-responses`; those pieces are unreleased scaffolding and must not shape either contract.

Implement callback ingress directly in the Attune API, update the canonical pre-production contract in place, and remove the scaffolding before release. Do not add compatibility formats, dual readers, data conversion, or a staged runtime cutover.

## Decision

Providers that deliver signed HTTP callbacks will call this public API route:

```text
POST /api/v1/inquiry-callbacks/{adapter_key}
```

The route will use a configured callback adapter to authenticate the request, decode the body, identify the external actor, extract an opaque response-option handle, and produce the provider's required acknowledgment. The adapter will then pass a provider-neutral selection to the inquiry response service.

The inquiry service will not contain branches for Slack, Discord, Microsoft Teams, or another provider. It will continue to own identity mapping, assignment checks, RBAC, response validation, state changes, audit records, and workflow notification.

Persistent provider transports use the managed-sensor path. A fenced sensor owns the authenticated provider connection and forwards the native envelope to `POST /api/v1/internal/inquiry-callbacks/{adapter_ref}`. The sensor's release-pinned `config.inquiry_callback_adapters` metadata selects bounded JSON Pointer extraction and request constraints. The API rechecks the current workload fence and persists an encrypted delivery before acknowledgment. The API then owns that delivery independently of the sensor process lease. Its background monitor invokes the same inquiry response service and retries deliveries left pending by API crashes. Callers cannot choose the provider, integration identity, actor kind, or response body because those values come from authenticated sensor state and pinned metadata.

Provider protocols differ enough that one Slack-shaped HMAC configuration cannot represent them safely:

- Slack signs `v0:{timestamp}:{raw_body}` with HMAC-SHA256 and sends a form field that contains JSON.
- Discord signs `{timestamp}{raw_body}` with Ed25519, sends JSON, and requires signed `PING` requests to receive a `PONG` response.
- Microsoft Teams uses the Bot Framework Activity protocol. Bot Framework authenticates requests with a bearer JWT verified through OpenID metadata and JWKS. Teams `Action.Execute` callbacks require a typed `invoke` response envelope.

The callback adapter interface must therefore separate authentication, decoding, normalization, acknowledgment, and continuation. Each implementation must satisfy the same provider-neutral contract. The inquiry route and repository must not know which implementation ran.

## Goals

- Do not ship the standalone Slack inquiry callback listener or normalized bearer endpoint.
- Let providers with signed HTTP callbacks call the Attune API directly.
- Keep provider authentication and wire formats outside inquiry domain logic.
- Define fixed response options on an inquiry and validate each option against the inquiry's flat `response_schema`.
- Bind each provider control to one compact, opaque response-option handle.
- Meet provider acknowledgment deadlines without an in-memory delivery queue.
- Preserve exact external identity mapping and first-writer-wins inquiry semantics.
- Make callback acceptance durable before Attune acknowledges the provider.
- Keep raw callback bodies, signatures, credentials, continuation tokens, and response handles out of logs and audit details.

## Non-goals

- This plan does not move outbound provider message delivery into the API. Pack actions still create provider messages and use provider idempotency controls.
- This plan does not make the ordinary trigger webhook receiver mutate inquiries.
- This plan does not define a general-purpose expression language for cryptographic verification.
- This plan does not accept arbitrary executable callback code from a pack.
- This plan does not add delegated Microsoft Graph access or Teams user SSO. Bot transport authentication and delegated user authentication are separate concerns.
- The first delivery supports fixed response options. Provider forms that construct arbitrary response objects can use the same interface later, but they need a separate bounded field-mapping design.

## Why `response_schema` is not enough

`response_schema` validates the final response object. It does not describe an interaction or authenticate its source.

For example, this schema permits a valid approval response:

```json
{
  "approved": {
    "type": "boolean",
    "required": true
  },
  "reason": {
    "type": "string",
    "required": true
  }
}
```

The schema does not say:

- which controls a provider should render;
- which response each control selects;
- how the provider signs a callback;
- how to decode the callback body;
- where to find the tenant and actor;
- how to acknowledge the provider;
- how to deduplicate delivery;
- which integration identity owns the callback adapter.

Keep response validation and interaction semantics separate. Add `response_options` to the inquiry creation contract.

```json
{
  "purpose": "production-deploy",
  "prompt": "Approve production deployment?",
  "response_schema": {
    "approved": {
      "type": "boolean",
      "required": true
    },
    "reason": {
      "type": "string",
      "required": true
    }
  },
  "response_options": [
    {
      "ref": "approve",
      "label": "Approve",
      "style": "positive",
      "response": {
        "approved": true,
        "reason": "Approved"
      }
    },
    {
      "ref": "reject",
      "label": "Reject",
      "style": "destructive",
      "response": {
        "approved": false,
        "reason": "Rejected"
      }
    }
  ],
  "assigned_to": 42,
  "timeout_seconds": 3600
}
```

Attune must reject inquiry creation unless every option:

- has a unique `ref`;
- has a non-empty label within the platform limit;
- uses a supported provider-neutral style;
- contains a response object that passes the flat `response_schema`;
- stays within configured option-count and serialized-size limits.

`response_options` is immutable after creation and participates in the existing idempotency comparison.

## Compact option-bound handles

Use one response handle per response option. Do not expose the inquiry-only handle from the unreleased scaffolding as a public contract.

Each handle must bind:

- the handle format version;
- the inquiry ID;
- the response-option index or stable option identifier;
- the inquiry-response audience.

The handle supplies correlation and option integrity. It does not replace callback authentication, identity mapping, RBAC, assignment checks, or the pending-state compare-and-set.

Discord limits component `custom_id` and select option values to 100 characters. Use a compact binary payload, authenticated encryption, URL-safe Base64 without padding, and a short ASCII prefix. Set a hard maximum of 96 ASCII characters so every supported provider can carry it without another lookup token.

A fixed-size payload can remain well below that limit. For example, a one-byte version, an eight-byte inquiry ID, and a two-byte option index produce a compact handle after adding the AES-GCM nonce and tag. Do not embed the response object, labels, actor data, or provider data in the handle.

Equivalent inquiry-creation retries may issue different valid handles for the same option. Every handle becomes unusable when the inquiry leaves `pending`.

## Callback adapter model

Add an `inquiry_callback_adapter` model with these responsibilities:

- route a public callback through an opaque `adapter_key`;
- bind the callback to one integration identity;
- assign the provider name used by external identity mappings;
- select typed authentication, decoder, normalizer, acknowledgment, and continuation configurations;
- enforce request-size, rate, and processing-time limits;
- reference secrets through Attune keys rather than inline plaintext;
- support enable, disable, key rotation, and audit-safe administration.

The public `adapter_key` routes the request. It is not proof that the provider sent the request.

Use typed Rust enums for adapter configuration. Reject unknown fields. Do not interpret an unrestricted JSON template as authentication logic.

Store provider, integration identity, and typed protocol configuration in immutable adapter revisions. The adapter row points to its active revision. Configuration changes and key rotation create a new revision instead of mutating one that may have authenticated queued work. Every callback delivery pins the revision that authenticated it.

Add `Resource::InquiryCallbackAdapters` with the existing create, read, update, and delete actions. Grant those actions to `core.admin`. Adapter creation or reassignment must also authorize `identities:update` against the selected integration identity. This prevents an adapter administrator from borrowing a more privileged identity's `inquiries:respond` grants.

When adapter configuration references a secret Attune key, create and update must authorize both the caller and the bound integration identity for `keys:read` and `keys:decrypt` against that exact key and its ownership constraints. Persist the key ID and its `updated` value as the credential revision marker, not the secret value. Runtime resolution must repeat authorization for the integration identity and reject a missing, disabled, changed, or no-longer-readable key rather than silently adopting new credential material. Rotating a credential updates the key and creates a new adapter revision in one transaction. The API service must never use its own authority to turn an unauthorized key reference into a signing credential. Public verification keys do not require decrypt authority when stored in a non-secret typed field.

### Authentication interface

The authentication stage receives request headers, the exact raw body, the current time, and typed adapter configuration. It returns an authenticated request context or a rejection. Authentication must finish before body normalization trusts actor or tenant fields.

The first contract needs these strategies:

#### HMAC signed message

Configuration includes:

- algorithm;
- secret key reference;
- signature header;
- signature encoding and optional prefix;
- a bounded sequence of signed components;
- optional timestamp header and maximum age.

Allowed signed components are `literal`, `header`, and `raw_body`. Bound the component count and literal length. This strategy represents Slack without adding Slack branches to the inquiry service.

#### Detached Ed25519 signature

Configuration includes:

- public key or public-key reference;
- signature header and encoding;
- a bounded sequence of signed components.

This strategy represents Discord's timestamp plus raw-body signature.

#### OpenID/JWKS bearer JWT

Configuration includes:

- allowed OpenID metadata origin or an administrator-approved cloud profile;
- required issuer;
- required audience;
- accepted algorithms;
- clock skew;
- required claim-to-body equality checks;
- optional signing-key endorsement checks;
- bounded JWKS cache and refresh policy.

This strategy represents Bot Framework authentication. The implementation must validate the bearer scheme, JWT signature, issuer, audience, validity window, `serviceUrl` binding, and channel endorsements. It must refresh an unknown signing key once before rejection and refresh cached keys at least as often as the configured provider contract requires.

Do not allow arbitrary metadata URLs from pack YAML. Resolve only administrator-approved HTTPS origins to prevent server-side request forgery. Government and sovereign cloud endpoints require separate approved profiles.

### Decoder interface

The decoder runs only after authentication. The first contract needs:

- `json`: decode the raw body as one JSON value;
- `form_json`: parse a bounded form body and decode one named field as JSON.

The decoder must reject duplicate security-sensitive form fields, excessive nesting, oversized values, invalid UTF-8 where the protocol requires UTF-8, and trailing data.

Slack uses `form_json` with the `payload` field. Discord and Bot Framework use `json`.

### Normalizer interface

The normalizer converts the authenticated provider document into this internal shape:

```text
NormalizedInquiryCallback {
    adapter_id,
    integration_identity_id,
    provider,
    provider_delivery_id,
    response_handle,
    external_actor: {
        tenant,
        subject_kind,
        external_subject,
    },
    continuation,
}
```

Configuration may use bounded exact-match predicates and JSON Pointer candidates. It must not execute arbitrary expressions.

The normalizer needs:

- request-class predicates, such as Discord `type == 3` or Teams `type == "invoke"` and `name == "adaptiveCard/action"`;
- ordered fallback pointers, such as Discord `member.user.id` followed by `user.id`;
- a static provider value from the adapter, never from the request;
- tenant extraction with explicit fallbacks;
- ordered subject candidates that each pair a configuration-owned `subject_kind` with one JSON Pointer;
- response-handle extraction;
- optional delivery-ID extraction;
- optional continuation extraction into encrypted, short-lived storage.

Add `subject_kind` to external actor assertions and external identity mappings. Teams can supply either a Microsoft Entra object ID or a bot-scoped Teams user ID. Treating both as the same untyped string invites incorrect mappings. The request can never supply or override `subject_kind`; the normalizer selects the kind attached to the first subject candidate whose pointer resolves.

The normalizer must preserve provider IDs as strings. Discord snowflakes exceed JavaScript's safe integer range.

### Control-request interface

Some authenticated callbacks configure or maintain the provider endpoint rather than answer an inquiry. The adapter must support exact-match control requests with static, bounded responses.

Discord sends a signed `type: 1` `PING` request while registering an interaction endpoint. The configured control response must return HTTP 200, `Content-Type: application/json`, and `{"type":1}`. A control request never reaches the inquiry service. Routine security checks may instead carry intentionally invalid signatures, which authentication must reject.

### Acknowledgment interface

The acknowledgment stage converts an ingress outcome into the provider's required HTTP response. It must support:

- status code;
- content type;
- a bounded static JSON or text body;
- typed fields from a small allowlist of safe outcome values;
- immediate, deferred, duplicate, invalid-selection, and internal-failure outcomes.

Do not use an unrestricted response template language.

The three initial profiles need different responses:

- Slack needs HTTP 200 within three seconds. A button action can use an empty body.
- Discord needs a typed interaction response within three seconds. It can return a deferred component update while durable processing continues.
- Teams `adaptiveCard/action` needs outer HTTP 200 and an inner invoke envelope containing `statusCode`, `type`, and `value`.

Authentication failures bypass normal acknowledgment and return the status required by the authentication strategy.

### Continuation interface

A provider may require work after the initial acknowledgment:

- Slack may provide a `response_url` that can update the source message.
- Discord provides an interaction token valid for 15 minutes for editing the initial response or sending follow-ups.
- Teams provides a validated `serviceUrl`, conversation ID, and activity references for replies or card updates.

The first implementation may acknowledge without updating the provider message. The interface must still reserve a typed continuation driver so later work does not leak provider fields into inquiry records.

Continuation values are capabilities. Encrypt them at rest, give them explicit expiration and use budgets, never log them, and delete them after final use or expiration. Slack `response_url` state must enforce Slack's limit of five uses within 30 minutes.

A Teams continuation driver also needs outbound Bot Connector authentication. Its typed configuration must select an approved cloud profile and one supported credential source: a client credential key reference or a user-assigned managed identity. The driver obtains and caches a bot access token for the cloud profile's Connector scope before it sends a reply or card update. Inbound Bot Framework JWT validation does not authorize outbound Connector calls.

A Teams continuation driver must not call `serviceUrl` unless Bot Framework JWT validation proved that the body and token contain the same URL. It must send the bot access token only to a validated Connector origin.

## Durable callback inbox

Use a database-backed `inquiry_callback_delivery` inbox. Do not carry the unreleased listener's memory queue into the released architecture.

The ingress route performs this sequence:

1. Load the enabled adapter by `adapter_key`.
2. Enforce the body-size and rate limits.
3. Authenticate the exact raw request.
4. Decode and classify the request.
5. Return a configured control response when the request is a control request.
6. Normalize an inquiry callback.
7. Compute a request digest and a provider delivery key when available.
8. Insert or claim the normalized callback in the inbox with a uniqueness constraint.
9. Attempt the inquiry response and terminal inbox transition in one database transaction within a strict provider-specific time budget.
10. Return an immediate or deferred acknowledgment.
11. Let a background processor finish any accepted delivery that remains pending.

The inbox stores only the normalized callback needed for processing. Encrypt the response handle and continuation capabilities. Do not store the raw body, authorization header, signature, signing input, provider token, or arbitrary decoded payload.

Suggested states are `pending`, `processing`, `accepted`, `duplicate`, `rejected`, and `failed`. A retry claims a row with `FOR UPDATE SKIP LOCKED` or the repository's existing claim pattern. Every transition must be idempotent.

Use the provider delivery ID when the protocol supplies one:

- Discord: interaction `id`.
- Teams: Activity `id`, scoped by adapter and conversation or tenant as required.
- Slack: no universal interactive callback ID exists. Use a digest of the authenticated raw request and bounded timestamp window as the delivery key.

The delivery claim, inquiry state transition, accepted external attribution, and terminal delivery state must commit in one transaction. Store the callback delivery ID in the allowlisted external attribution. If a worker retries after losing its connection, it can then distinguish its own committed response from a different option that won. The inquiry pending-state compare-and-set remains the final duplicate-response guard.

## Shared inquiry response service

Build one internal inquiry response service from the common transaction logic in the human-response path and the unreleased external-response scaffolding. The human response route and direct callback ingress call it; the normalized external-response route does not ship.

The service accepts typed human or callback-adapter provenance, an optional callback delivery ID, and a normalized option selection. It must:

1. Resolve and authenticate the option-bound handle.
2. Load the pinned callback adapter revision for provider callbacks, or the authenticated human identity for the human route.
3. perform a fresh `inquiries:respond` authorization check for the integration identity.
4. Resolve the exact external identity mapping by integration identity, provider, tenant, subject kind, and external subject.
5. Require the mapped identity to equal `assigned_to`.
6. Acquire the workflow advisory lock.
7. Reject cancelling or terminal workflows.
8. Lock the inquiry and require `pending` before timeout.
9. Load the immutable response option.
10. Validate the stored option response against the flat `response_schema` again.
11. Persist the response, mapped identity, callback delivery ID, and allowlisted external attribution with the existing compare-and-set.
12. Mark the callback delivery terminal in the same transaction when a delivery ID is present.
13. Commit before emitting audit and RabbitMQ notifications.

The callback ingress binds directly to the configured integration identity. It does not store an integration token or call `/auth/token-login` through loopback HTTP.

Remove the unreleased `POST /api/v1/inquiry-responses` route and its integration-token login dependency. If external adapter SDKs become a requirement later, specify and authorize that integration path separately rather than retaining scaffolding as an accidental API.

Move the existing execution-ancestry query used by the human response privilege-loop check behind `ExecutionRepository` while extracting this service. Every response path must continue to use repository-owned database access.

## Provider conformance

The adapter interfaces must satisfy this matrix before inquiry callbacks are released.

| Requirement | Slack | Discord | Microsoft Teams | Required interface capability |
|---|---|---|---|---|
| Incoming body | Form field containing JSON | JSON interaction | JSON Bot Framework Activity | `form_json` and `json` decoders |
| Request authentication | HMAC-SHA256 shared secret | Ed25519 detached signature | RS256 bearer JWT through OpenID/JWKS | Typed authentication strategies |
| Signed material | `v0:` + timestamp + raw body | Timestamp + raw body | JWT claims and signature, plus body claim binding | Bounded signed components and JWT semantic checks |
| Replay or duplicate input | Five-minute timestamp window; no universal interaction ID | Interaction ID; no documented timestamp-age rule | Activity ID; JWT validity window | Freshness checks and durable delivery keys |
| Endpoint control request | None for interactions | Signed `PING`, return `PONG` | Bot endpoint registration occurs outside the callback body | Authenticated control responses |
| Actor | `user.id` | `member.user.id` or `user.id` | Prefer `from.aadObjectId`; fallback `from.id` is bot-scoped | Fallback extraction and `subject_kind` |
| Tenant or installation | `team.id`, with enterprise-install cases | Guild or authorizing installation owner | `channelData.tenant.id` or conversation tenant | Fallback extraction without numeric coercion |
| Selection carrier | Button or option value | `data.custom_id` or selected value | `value.action.data` for `Action.Execute`; `value` for `Action.Submit` | Compact option handle and request-class normalizer |
| Immediate response | Empty HTTP 200 | Typed interaction callback | HTTP 200 with typed invoke body for `Action.Execute` | Outcome-aware acknowledgment |
| Published deadline | Three seconds | Three seconds | No numeric inbound deadline in the cited public contract; invoke response is synchronous | Per-adapter processing budget and durable inbox |
| Follow-up capability | Optional `response_url` | Interaction token valid for 15 minutes | Connector API through validated `serviceUrl` | Encrypted, expiring continuation driver |
| Smallest handle limit used here | Slack option values allow 150 characters | `custom_id` and option values allow 100 characters | Card action data carries JSON | Maximum 96 ASCII characters |

### Slack conformance notes

Slack sends interactive requests as `application/x-www-form-urlencoded`; the `payload` form field contains JSON. Slack requires HMAC-SHA256 verification over `v0:{timestamp}:{raw_body}`, using `X-Slack-Request-Timestamp` and `X-Slack-Signature`. Slack's documentation uses a five-minute timestamp window and requires HTTP 200 within three seconds.

The normalizer must not assume that every Slack control has `actions[0].value`. Different controls use fields such as selected options, selected users, or selected conversations. The first approval profile can require a button or fixed option value, but the decoder and request classifier must remain extensible.

Official sources:

- [Verifying requests from Slack](https://docs.slack.dev/authentication/verifying-requests-from-slack/)
- [Handling user interaction in Slack apps](https://docs.slack.dev/interactivity/handling-user-interaction/)
- [Block actions payload](https://docs.slack.dev/reference/interaction-payloads/block_actions-payload/)
- [Button element](https://docs.slack.dev/reference/block-kit/block-elements/button-element/)
- [Option object](https://docs.slack.dev/reference/block-kit/composition-objects/option-object/)

### Discord conformance notes

Discord sends JSON interactions and requires Ed25519 verification of `X-Signature-Timestamp` followed by the exact raw body. Discord sends intentionally invalid signatures during routine endpoint checks and may remove an endpoint that accepts them.

Endpoint registration sends a signed interaction with `type: 1`. Attune must return a JSON `PONG` with `type: 1`. Routine security checks can instead use intentionally invalid signatures. Component interactions use `type: 3`; component data includes `custom_id`, component type, and optional selected values. Guild interactions identify the actor under `member.user`; direct-message interactions use `user`.

Discord invalidates the interaction token unless the initial response arrives within three seconds. The token remains usable for follow-up operations for 15 minutes after a valid initial response. Component `custom_id` and string-select values are limited to 100 characters, which sets the handle ceiling in this plan.

Official sources:

- [Discord interaction endpoint setup and signature verification](https://docs.discord.com/developers/interactions/overview#preparing-for-interactions)
- [Discord interaction objects and responses](https://docs.discord.com/developers/interactions/receiving-and-responding)
- [Discord component `custom_id` and component limits](https://docs.discord.com/developers/components/reference#anatomy-of-a-component-custom-id)
- [Discord HTTP content types](https://docs.discord.com/developers/reference#http-api)

### Microsoft Teams conformance notes

Teams bot callbacks use Bot Framework Activities. The request has a bearer JWT. Authentication requires Bot Framework OpenID metadata and signing keys, an RS256 signature, issuer and audience checks, token lifetime checks, `serviceUrl` claim equality, and channel endorsement checks where required. Microsoft says to refresh signing keys at least once every 24 hours.

Teams approvals should prefer Adaptive Card `Action.Execute`. The outbound Teams pack must emit Adaptive Card schema version 1.5. It arrives as an `invoke` Activity named `adaptiveCard/action`. The selected action appears under `value.action`, with the operation in `verb` and hidden data plus card input values in `data`. The response must use outer HTTP 200 and an invoke body with `statusCode`, `type`, and `value`. An updated Adaptive Card can replace the current card.

Every outbound `Action.Execute` must include an `Action.Submit` fallback for older Teams clients, and the Teams profile must process both actions. `Action.Submit` cannot return the same immediate replacement-card response as `Action.Execute`. Before implementation, pin an official Microsoft fixture or SDK contract for the fallback Activity's exact wire shape instead of inferring it from `Action.Execute`.

Use the validated tenant and actor fields for identity mapping. Prefer `channelData.tenant.id` and `from.aadObjectId`. If only `from.id` exists, record a distinct subject kind because Microsoft scopes that identifier to the user and bot identity rather than treating it as a global Microsoft user ID.

Official sources:

- [Authenticate requests with the Bot Connector API](https://learn.microsoft.com/en-us/azure/bot-service/rest-api/bot-framework-rest-connector-authentication?view=azure-bot-service-4.0)
- [Bot Framework Activity schema](https://learn.microsoft.com/en-us/azure/bot-service/rest-api/bot-framework-rest-connector-api-reference?view=azure-bot-service-4.0#activity-object)
- [Universal Action request and response format](https://learn.microsoft.com/en-us/adaptive-cards/authoring-cards/universal-action-model)
- [Use Universal Actions in Teams](https://learn.microsoft.com/en-us/microsoftteams/platform/task-modules-and-cards/cards/universal-actions-for-adaptive-cards/work-with-universal-actions-for-adaptive-cards)
- [Teams card actions](https://learn.microsoft.com/en-us/microsoftteams/platform/task-modules-and-cards/cards/cards-actions)
- [Teams proactive messaging and user identity](https://learn.microsoft.com/en-us/microsoftteams/platform/bots/how-to/conversations/send-proactive-messages)

## Relationship to the trigger webhook receiver

The current `POST /api/v1/webhooks/{webhook_key}` route creates events. It expects an Attune JSON envelope and supports conventional HMAC headers over the raw body. It does not support Slack's signed-message format, Discord Ed25519 signatures and `PING`, or Bot Framework bearer JWT validation. It also does not perform an inquiry state transition.

Do not overload that endpoint. Reuse its proven concepts and extract shared code where the behavior is truly identical:

- raw-body limits;
- IP policy;
- database-backed rate limiting;
- constant-time comparison;
- ingress outcome logging;
- opaque route keys.

Keep trigger webhook delivery and inquiry callback delivery as separate models, repositories, routes, audit events, and authorization paths.

## Data model

### Inquiry changes

Add immutable `response_options JSONB` to `inquiry`. Validate it at the API boundary and repository boundary. Include it in idempotency equivalence checks. Do not return response handles from ordinary inquiry read or list endpoints.

Creation returns rendering metadata and one handle per option:

```json
{
  "data": {
    "inquiry": {
      "id": 918,
      "status": "pending"
    },
    "response_options": [
      {
        "ref": "approve",
        "label": "Approve",
        "style": "positive",
        "response_handle": "attune_irh_REDACTED"
      },
      {
        "ref": "reject",
        "label": "Reject",
        "style": "destructive",
        "response_handle": "attune_irh_REDACTED"
      }
    ]
  }
}
```

The action may use labels and styles to render provider controls. The action must not return handles in its action result or write them to logs.

### External identity mapping changes

Add non-null `subject_kind` to `external_identity_mapping` and `external_actor`. Update the unique key to cover:

```text
integration identity + provider + tenant + subject kind + external subject
```

Update every create, update, list, exact-resolution, DTO, OpenAPI, generated-client, repository, and test path in the same implementation wave. Revise the pre-production schema in place. No runtime dual reader or data-conversion path is needed.

### Callback adapter

Create `inquiry_callback_adapter` with a `BIGSERIAL` primary key, unique ref, hashed route key, lifecycle state, active revision, and audit timestamps. Create `inquiry_callback_adapter_revision` with a `BIGSERIAL` primary key, adapter ID, provider, integration identity, immutable typed configuration, and creation metadata. Store secret references rather than secret values.

Management routes require `RequireAuth`, `inquiry_callback_adapters:*` RBAC, authority over the bound integration identity, and key-specific read and decrypt authority for every credential reference. Read responses must redact the route key after creation and must never return resolved secrets.

Deleting an adapter means retiring it, not deleting its row. Retirement disables new ingress and key rotation but lets deliveries already authenticated under a pinned revision finish. Explicit revocation disables ingress, invalidates unused continuation capabilities, and causes pending deliveries to terminate without changing an inquiry. The inquiry response transaction locks the adapter lifecycle row while checking it; revocation takes the conflicting write lock and invalidates continuations in the same transaction. Once revocation commits, no callback response or continuation for that adapter can commit. Use `ON DELETE RESTRICT` from revisions to adapters and from callback deliveries to revisions. The supervisor may purge a retired or revoked adapter only after no active or retained delivery and no unexpired continuation references it.

### Callback delivery

Create `inquiry_callback_delivery` with a `BIGSERIAL` primary key, adapter revision ID, immutable integration identity and provider provenance, provider delivery key, request digest, state, encrypted normalized payload, attempt count, sanitized error classification, lease fields, and timestamps.

Add uniqueness constraints that make repeated provider delivery converge on one row. Retention belongs to the supervisor. Cleanup must operate on owned terminal rows and must not delete active deliveries by age alone.

## API and module placement

Suggested files:

```text
crates/common/src/repositories/inquiry_callback_adapter.rs
crates/common/src/repositories/inquiry_callback_delivery.rs
crates/api/src/routes/inquiry_callbacks.rs
crates/api/src/inquiry_callbacks/authentication.rs
crates/api/src/inquiry_callbacks/decoder.rs
crates/api/src/inquiry_callbacks/normalizer.rs
crates/api/src/inquiry_callbacks/acknowledgment.rs
crates/api/src/inquiry_callbacks/processor.rs
crates/api/src/inquiry_response.rs
```

Keep the public Axum handler shallow. It coordinates raw request capture, adapter lookup, limits, the typed callback pipeline, inbox persistence, and acknowledgment. Repository modules own every database query.

API replicas own the callback delivery processor because the API owns callback ingress and provider acknowledgment. Start one processor task per API instance with the service cancellation token. Use repository leases and `FOR UPDATE SKIP LOCKED` so replicas share work safely. On shutdown, stop new claims, await active delivery transitions, join the processor task, and then close the database pool. Lease expiry recovers work from an ungraceful process exit.

Register callback and management route modules in `routes/mod.rs` and `server.rs`. Add every path and schema to `openapi.rs`, then regenerate both TypeScript and Python clients.

If provider profile constructors are useful, place them outside the inquiry service. A profile is data that composes the typed interfaces. It is not permission to add `if provider == "slack"` to callback routing or inquiry state transitions.

Add `inquiry_callback_delivery` to the persisted retention-target enum and supervisor configuration. Implement candidate counts, dependency ordering, and deletion through the repository layer. Retention may delete only terminal, unleased deliveries and expired continuation capabilities.

## Security requirements

- Require HTTPS for every public callback URL.
- Bound request bodies before parsing.
- Authenticate exact raw bytes before decoding when the provider contract requires it.
- Compare MACs and signatures with established cryptographic libraries.
- Fail closed on unknown algorithms, headers, claims, request classes, fields, and acknowledgment outcomes.
- Fetch OpenID metadata and JWKS only from administrator-approved HTTPS origins.
- Bound and cache key discovery. Refresh an unknown key once, then reject.
- Treat provider continuation URLs and tokens as capabilities.
- Validate Bot Framework `serviceUrl` binding before any Connector call.
- Preserve provider IDs as strings.
- Add `subject_kind` to prevent cross-namespace identity mapping.
- Recheck the pinned adapter revision, lifecycle policy, integration identity state, RBAC, mapping state, and assignee state when processing an inbox row. A retired adapter permits already-accepted work; a revoked adapter does not.
- Lock the adapter lifecycle row through the inquiry response commit so revocation is a transactional fence, not a stale preflight check.
- Require adapter administrators to hold authority over the bound integration identity and every referenced key.
- Never trust actor, tenant, response option, or continuation fields before callback authentication.
- Never persist raw callback bodies or authorization headers.
- Never put response handles, signatures, tokens, response content, or decrypted secrets in logs or audit details.
- Rate-limit before expensive cryptographic or JSON work where possible without creating an authentication oracle.
- Keep one bounded processing deadline per adapter. Do not let a provider request occupy an API worker indefinitely.

## Delivery sequence

This is an implementation order, not a deployment migration. Nothing in this subsystem has a released runtime contract or persisted production data to preserve.

### Phase 1: establish the domain contract

- Delete the standalone Slack listener, integration-token login, memory queue, retry loop, listener deployment instructions, inquiry-only handle, and normalized external-response route before building the canonical path.
- Add immutable `response_options` to inquiry creation and persistence.
- Validate static responses against the flat schema.
- Implement compact option-bound handles no longer than 96 ASCII characters and return them in the initial creation contract.
- Add `subject_kind` to external actor assertions, mappings, uniqueness constraints, repositories, DTOs, and generated clients.
- Put the execution-ancestry query behind `ExecutionRepository`.
- Build one shared inquiry response service for human and callback-adapter provenance. Keep fresh RBAC, mapping, assignment, workflow locking, schema validation, compare-and-set, audit, and publication in that service.
- Update idempotency comparison and tests, revise the pre-production migrations in place, and run `cargo sqlx prepare`.

Exit condition: user and option responses share state-transition tests, while option substitution, tampering, oversized handles, stale handles, and duplicate responses fail.

### Phase 2: build direct callback ingress

- Add adapter and immutable revision repositories, callback-delivery repositories, and management APIs.
- Add `Resource::InquiryCallbackAdapters`, grant it to `core.admin`, and enforce integration-identity and key-specific authorization during adapter writes.
- Add HMAC, Ed25519, and OpenID/JWKS authentication strategies.
- Add JSON and form-JSON decoders.
- Add bounded predicates, pointer fallbacks, control responses, and outcome acknowledgments.
- Add the API-owned, lease-based inbox processor and supervisor retention target.
- Register routes and OpenAPI schemas, regenerate TypeScript and Python clients, and run `cargo sqlx prepare`.

Exit condition: a process crash after acknowledgment does not lose an accepted callback.

### Phase 3: add provider profiles and pack integration

- Build a Slack profile from the typed configuration.
- Build a Discord profile, including signed `PING` and deferred component acknowledgment.
- Build a Bot Framework profile, including JWT claims, key rotation, endorsements, `serviceUrl` binding, `Action.Execute`, and `Action.Submit` fallback.
- Update provider actions to create inquiries with response options and render the returned option-bound handles.
- Keep outbound provider message delivery in pack actions.

Exit condition: no provider callback process exists outside Attune, and each provider pack contains only outbound delivery logic.

### Phase 4: conformance and live acceptance

- Test a Teams pack fixture that emits Adaptive Card schema 1.5 and includes an `Action.Submit` fallback on every `Action.Execute`.
- Use official sample payload shapes and locally generated valid and invalid signatures or tokens.
- Test duplicate delivery, stale credentials, frozen identities, mapping mismatch, timeout, cancellation, and concurrent selections for every profile.
- Configure provider callback URLs directly against the Attune API and run live response, timeout, cancellation, retry, duplicate, recovery, and UI wait-link checks.

Exit condition: all three profiles pass the same provider-neutral inquiry acceptance suite plus their protocol-specific conformance suite, and the Slack example passes live acceptance without a standalone listener.

## Acceptance criteria

- Slack, Discord, and Teams callbacks enter through the same Axum route and callback pipeline.
- The inquiry service and repositories contain no provider-specific branches.
- Every response option is validated at inquiry creation and selected through an option-bound handle.
- Every handle fits Discord's 100-character component limit.
- Invalid Slack HMAC, Discord Ed25519, and Bot Framework JWT requests fail before actor extraction.
- Discord endpoint `PING` validation succeeds and invalid signature probes return 401.
- Bot Framework validation checks issuer, audience, lifetime, signature, `serviceUrl`, and required channel endorsements.
- Slack and Discord acknowledgments meet their three-second published deadlines under the configured load budget.
- Teams `Action.Execute` receives outer HTTP 200 and a valid invoke response envelope.
- The inbox makes provider acknowledgment durable across API process failure.
- The inbox delivery transition and inquiry response commit atomically, so lease recovery can identify its own completed response.
- Duplicate callback delivery and concurrent option selection produce one inquiry response.
- External identity mapping distinguishes subject namespaces through `subject_kind`.
- Adapter writes cannot bind an integration identity or secret key that the caller is not authorized to manage, and runtime key use requires fresh integration-identity authorization.
- No raw callback body, response handle, signature, bearer token, continuation token, or response content appears in logs or audit records.
- No standalone Slack inquiry callback listener or normalized external-response compatibility endpoint ships.

## Open implementation decisions

- Decide whether callback adapter configuration lives only in the database or can also be declared by trusted system-pack metadata.
- Set maximum response-option count and serialized sizes. The first provider profiles need no more than 25 options.
- Decide whether successful deferred callbacks update provider messages in the first release or only complete the Attune inquiry.
- Define approved Bot Framework cloud profiles for public Azure and any required sovereign clouds.
- Define the retention period for terminal callback deliveries and expired continuation capabilities.

## Source review

The provider requirements in this plan were checked against the official public documentation linked above on 2026-09-20. Protocol behavior changes over time. Recheck those sources while implementing each provider conformance profile and record the source revision or access date in its tests.
