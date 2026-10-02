# OIDC device login

`attune auth sso-login` uses the OAuth 2.0 Device Authorization Grant from
[RFC 8628](https://www.rfc-editor.org/rfc/rfc8628). The CLI makes outbound requests
to Attune. The API holds provider client credentials, obtains a device code,
and validates the provider's identity token after approval. The CLI receives
Attune access and refresh tokens, not provider tokens.

## Provider setup

The discovery document must include `device_authorization_endpoint`. Enable
`urn:ietf:params:oauth:grant-type:device_code` for the client registered in
`security.oidc`. Attune requests `openid email profile` and the configured scopes.
The provider must return an ID token. Existing browser authorization-code login
continues to use its registered callback URI and PKCE.

The CLI flow requires no redirect URI. The API's existing browser callback remains
configured for web login. Attune does not fall back to a local callback listener
when a provider lacks device support.

```yaml
security:
  oidc:
    enabled: true
    discovery_url: https://idp.example.com/.well-known/openid-configuration
    client_id: attune-web
    client_secret: your-server-held-secret
    redirect_uri: https://attune.example.com/auth/callback
    scopes: [groups]
```

`security.encryption_key` protects short-lived device authorization sessions. API
replicas must share that key, JWT configuration, and OIDC settings.

Some providers require a native application registration for device grants. An
explicit override uses that registration without inheriting the web secret:

```yaml
device_client:
  client_id: attune-native
  client_secret: null
```

The device token's audience must match the selected device client. After validation,
its issuer and subject bind to the primary client's Attune identity realm. The
actual device client appears in `attributes.oidc.authentication_client_id`.
Both registrations must issue compatible subject identifiers. Pairwise subject
identifiers can differ between registrations; Attune never links accounts by email.

## Group-based authorization

Configure the same lowercase `groups` claim and intended filters for the web and
native registrations. A native grant's UserInfo response uses the native app's
claim configuration, not the web app's filters.

Requesting the `groups` scope requires the claim for both browser and device
login. For providers that supply groups without that scope, set
`security.oidc.require_groups: true`. If neither condition applies, groups remain
optional. Attune accepts a string or an array containing only strings.

When required groups are missing, or group enrichment fails without verified
ID-token groups, Attune rejects login before creating or updating the identity
or changing its roles. A subject-mismatched UserInfo response cannot supply groups.
Malformed group claims reject login even when groups are optional.

An explicit empty ID-token list remains authoritative; UserInfo cannot replace it
with a nonempty list. Verified `groups: []` allows login and clears OIDC-managed
roles. A narrower nonempty list replaces those roles too. Direct permission
assignments and roles managed by other sources remain separate.

## Helm deployment

Chart 0.8.9 exposes the native registration and required-group setting:

```yaml
security:
  oidc:
    enabled: true
    discoveryUrl: https://example.okta.com/.well-known/openid-configuration
    clientId: WEB_CLIENT_ID
    redirectUri: https://attune.example.com/auth/callback
    scopes: [groups]
    deviceClient:
      clientId: NATIVE_CLIENT_ID
    requireGroups: false
  identitySecret:
    existingSecret: attune-identity
```

For Okta, enable Device Authorization on a Native application, set client
authentication to `none`, and assign the intended users. Keep the web registration
and exact issuer. A custom authorization server also needs an access-policy rule
that covers the native client, users, grant, and requested scopes.

Keep the web secret in the API-only identity Secret under
`ATTUNE__SECURITY__OIDC__CLIENT_SECRET`. Public native clients need no secret.
For a confidential device registration using `client_secret_basic`, supply its
own secret under `ATTUNE__SECURITY__OIDC__DEVICE_CLIENT__CLIENT_SECRET` in that
Secret. The chart writes no client secrets into its ConfigMap. Leave
`deviceClient.clientId` empty to reuse the primary registration.

Upgrade the API and CLI together to 0.7.4. Older APIs lack the device endpoints;
older CLIs use the removed loopback callback. See
[Okta compatibility research](../research/okta-device-login-compatibility.md)
for supported provider options and limits.

## CLI use

```bash
attune --profile production auth sso-login
attune auth sso-login --url https://attune.example.com --save-profile production --no-browser
attune --profile production auth sso-login --no-browser --timeout 300
```

Instructions go to stderr so JSON or YAML token output remains machine-readable.
The CLI always displays the verification URI and user code. Browser opening uses
`verification_uri_complete` when the provider supplies it. Approval can happen on
any machine. Ctrl+C cancels the wait, and failed logins preserve saved credentials.

Polling waits before the first request, defaults to five seconds when the provider
omits its interval, adds five seconds on each `slow_down`, and backs off on connection
timeouts. Denial, expiry, and other terminal errors stop polling.

## API endpoints

- `POST /auth/oidc/device/start` returns an opaque `device_code`, `user_code`,
  verification URLs, expiry, and polling interval.
- `POST /auth/oidc/device/poll` takes `{"device_code":"..."}` and returns a tagged
  waiting, authorized, denied, or expired response. Waiting responses carry the
  latest opaque session and interval. Authorized responses carry Attune tokens.

Both endpoints are pre-authentication routes and send `Cache-Control: no-store`.
The opaque session is encrypted, expires with the provider grant, and binds to
the current deployment and OIDC configuration. Provider grant redemption enforces
single use. Replaying an older encrypted session does not revoke it, so provider
polling enforcement still applies. If provider redemption succeeds but Attune
cannot finish or deliver authentication, start a new user-initiated login.

The userinfo subject must match the verified ID-token subject. Profile and group
enrichment follows the browser flow. Frozen identities cannot obtain tokens, and
an identity bound to an unrelated configured client cannot be adopted.
