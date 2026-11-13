# Webhook credentials and receiver trust

Trigger list/detail responses and expansions expose webhook enablement, never the
bearer key. Schema metadata visibility remains public for public triggers.
`POST /api/v1/triggers/{ref}/webhooks/reveal` requires an access credential and
current `triggers:update` authority for that exact target. The response is
credential-bearing and uses `Cache-Control: no-store`. The UI's “Reveal and copy
URL” invokes this explicit operation. API clients can call the same endpoint;
ordinary CLI trigger output no longer prints a key. Existing enable/regenerate
operations return a credential-bearing management response.

`referencing_pack_ref` is an authoring context, not an assertion clients may make
without authority: it requires `packs:configure` on the actual persisted pack.
This deliberate restriction is stronger than pack read. Omit it for ordinary
public schema reads; direct scoped trigger management visibility still applies.

## Receiver address

By default the receiver uses the actual socket peer, ignoring X-Forwarded-For and
X-Real-IP. Missing peer information fails closed. Behind operator-controlled
proxies configure `server.webhook_trusted_proxies` as an array (or comma-separated
list) of exact IP addresses/CIDRs. No networks are implicitly trusted. Proxies
must overwrite or append the actual upstream peer to a single X-Forwarded-For
header. The receiver validates the entire chain and walks right to left through
trusted hops, stopping at the first untrusted address. The resulting canonical IP
(including normalization of IPv4-mapped IPv6) drives both whitelist and rate-limit
checks. A trusted socket peer must supply a valid chain. Invalid proxy networks,
malformed security sections, unknown security sections, and enabled security
features with missing/invalid settings reject requests before event creation.

HMAC authenticates the raw request body independently of address checks. Enabled
HMAC requires a nonempty secret, supported algorithm, and valid signature.
Neither an allowed address nor a bearer URL bypasses this requirement.

## Controlled rotation

This change does not rotate credentials. Treat previously disclosed URL keys as
potentially copied. An authorized operator should inventory senders, schedule a
cutover, regenerate the key explicitly, distribute the new URL through a secret
channel, update senders, and verify delivery. The old URL stops working immediately;
there is no overlap window. Avoid logging URLs, credentials, or signature headers.
Update HMAC secrets separately through controlled pack configuration, coordinate
senders, and verify signed delivery. Do not reset databases or rotate live keys
as part of installing this code change. Integrate the administration authorization
fixes from PR #44 and lifecycle authorization from PR #43 before rollout.
