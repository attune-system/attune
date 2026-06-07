# Secret-safe operational metadata access

Implemented redacted-by-default handling for operational secrets across events, enforcements, executions, and notifier subscriptions.

## Changes

- Event ingress now redacts trigger-schema secret fields from event payload/config before storage.
- Event secret values are encrypted into `execution_secret_value` using entity types `event_payload` and `event_config`.
- `GET /api/v1/events/{id}?include_secret_values=true` restores event secrets only with `events:decrypt` and emits `secret.event_values.decrypted` audit events.
- Event/enforcement/execution list and detail reads now require the matching operational `read` permission for identity-scoped tokens.
- Enforcement payload remains a redacted copy of the stored event payload; enforcement decrypt restores only enforcement config.
- Notifier WebSocket subscriptions now require matching operational read grants for event/enforcement/execution filters; `all` requires admin or broad operational read.
- `core.admin` now includes operational decrypt grants; viewer/editor remain without operational decrypt grants.

## Validation

- `cargo test -p attune-common secret_values --lib`
- `cargo test -p attune-api routes::events::tests --lib`
- `cargo test -p attune-notifier websocket_server::tests::test_verify_ws_token_execution_ok`
- `cargo test -p attune-notifier test_filter_acl`
- `cargo check -p attune-common -p attune-api -p attune-notifier --tests`
