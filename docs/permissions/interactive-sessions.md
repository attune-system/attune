# Interactive sessions

Local registration/password login, LDAP and OIDC (including device login) issue
persisted interactive refresh sessions. Integration-token access/refresh retains
its separate credential-record provenance and revocation policy. Sensor, Worker
and recorded execution authority are unchanged.

Refresh rotates once under an identity/session transaction. Reuse of any prior
refresh credential revokes the entire family, committing before the rejection
response. Concurrent duplicate refresh calls produce at most one successful
rotation; the duplicate revokes that winner's refresh credential too. Clients
must serialize refresh and persist the replacement credential. A lost rotation
response requires signing in again. The family expires at its original refresh
expiry; rotation never extends it indefinitely.

`POST /auth/session/logout` accepts the session refresh credential and revokes
its family (also when the supplied credential was rotated). Browser cookie
logout also revokes the cookie's family. Password-hash changes revoke all
interactive families and outstanding browser exchanges atomically through the
identity trigger, including administrative resets. Identity deletion cascades;
frozen identities cannot refresh, issue sessions or redeem exchanges.

Interactive access JWTs are capped at five minutes at issuance. Already-issued
access tokens are not individually revoked on logout/password change: they may
remain usable until expiration, plus the shared validator's 60-second clock
leeway. Integration access TTL is unchanged. During rollout, preexisting access
JWTs retain their originally configured expiry; pre-session interactive refresh
JWTs have no backing record and fail closed, requiring a new login. Apply the
forward migration before deploying the API; no database reset is needed.

OIDC browser callback returns a random 60-second single-use exchange code, bound
to a random HttpOnly SameSite browser cookie. The SPA immediately removes the
code fragment then posts it to `/auth/exchange`. Only the stored, sanitized
redirect is returned. Access/refresh bearers never appear in callback URLs.
Exchange payloads are encrypted with the configured shared encryption key;
OIDC browser login requires that key. Fragments are not normally sent as HTTP
referrers; history and same-origin script access were the relevant exposure.

Expired/revoked session and exchange rows are retained until identity deletion;
operators should account for this metadata growth. Tokens are hashed in session
records. Exchange payload ciphertext is short-lived but currently retained after
expiry until identity deletion; successful redemption deletes it.

The browser UI requires Web Locks (a supported browser in a secure context) for
automatic refresh. Without it refresh fails before sending a credential; sign in
again in a supported environment. It never falls back to unsafe per-tab rotation.

Interactive JSON refresh updates HttpOnly auth cookies only when the incoming
refresh cookie exactly matches the submitted credential. Cookie-only refresh
also rotates cookies. JSON clients without cookies receive no cookies, and an
unrelated cookie session is never overwritten by an explicit body credential.
Integration refresh retains its separate existing cookie policy.
