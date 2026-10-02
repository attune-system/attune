# Okta device-login compatibility research

## Compatibility verdict

The current broker is likely compatible with Okta's documented device-flow setup:
a Native OIDC application, Device Authorization enabled, public client
authentication with `none`, signed RS256 ID tokens, and one consistent issuer for
web and CLI login. This is a documentation and source review, not an Okta tenant
test. The prior RDRX end-to-end validation used Authelia.

### Protocol and configuration comparison

| Okta option | Current Attune support |
| --- | --- |
| Native application with Device Authorization | Required by Okta. Use `oidc.device_client.client_id` when the main registration is a Web app. [D1] |
| Native public client, `token_endpoint_auth_method: none` | Supported. A null device-client secret sends only the client ID and never inherits the web secret. `oidc_device.rs:49-64,112-122`. [D1][D2] |
| `client_secret_basic` | Supported when the selected registration uses a secret. The API holds the secret. `oidc_device.rs:112-122`. [D2] |
| `client_secret_post`, `client_secret_jwt`, `private_key_jwt` | Not implemented. There is no configurable authentication-method selector, and supplying a secret always selects Basic authentication. [D2] |
| Org authorization server | Supported in principle using `https://ORG/.well-known/openid-configuration`. Its access token is used only with Okta UserInfo; Attune authenticates the ID token and issues its own API JWTs. [D3] |
| Custom authorization server | Supported in principle using `https://ORG/oauth2/SERVER_ID/.well-known/openid-configuration`. The native app needs a matching policy that permits Device Authorization and its requested scopes. Production entitlement applies. [D1][D3] |
| Device response fields and expiry | Okta documents `device_code`, `user_code`, both verification URLs, `expires_in: 600`, and `interval: 5`. The parser accepts these values and the CLI displays the base URL and user code. `oidc_device.rs:32-41,202-255`. [D1] |
| Token request | Form POST contains `client_id`, `grant_type=urn:ietf:params:oauth:grant-type:device_code`, and the provider device code. `oidc_device.rs:303-307`. [D1] |
| Pending and terminal responses | Handles `authorization_pending`, `slow_down`, `access_denied`, and `expired_token`. Slowdown increases the interval by five seconds. `oidc_device.rs:258-274,321-330`. [D1] |
| Signed RS256 ID tokens and JWKS rotation | Supported. Signature, discovered issuer, actual selected audience, expiry, applicable nonce, authorized party, and issue time are checked. JWKS is fetched for verification. `oidc.rs:702-773`. [D4] |
| Encrypted ID tokens | Not supported. No client decryption key or JWE decryption path is configured. Leave Okta ID-token encryption disabled. [D5] |
| Required DPoP | Not supported. The broker emits no DPoP proof, does not handle `use_dpop_nonce`, and uses Bearer UserInfo requests. Leave DPoP requirements disabled for this integration. [D6] |

An ordinary Okta Web or Browser registration cannot be assumed to support the
device grant. Okta's device guide explicitly restricts it to Native applications.
The separate-device-client override is therefore important for an existing web
SSO deployment. [D1]

### Required Okta setup

1. Create or select a Native OIDC app and enable Device Authorization. Assign the
   intended users to that app. Keep the existing web app for browser sign-in. [D1]
2. Choose `none` for the public native client. Set its device-client secret to null
   in Attune. If secret authentication is required, select Basic, which is the
   implemented secret method. [D2]
3. For a custom server, include the native app, users, Device Authorization grant,
   and requested scopes in a matching access policy and rule. [D1][O6]
4. Use the same exact authorization-server issuer for both registrations. The org
   server and `/oauth2/default` are distinct issuers. Custom domains and dynamic
   issuer mode must remain consistent with the chosen discovery URL. [D3][D7]
5. Configure the lowercase `groups` claim and intended filters for both apps, or
   the corresponding custom-server claim conditions. See the claim analysis below.

Application configuration for an existing confidential web client:

```yaml
security:
  oidc:
    enabled: true
    discovery_url: https://example.okta.com/.well-known/openid-configuration
    client_id: WEB_CLIENT_ID
    client_secret: WEB_CLIENT_SECRET
    redirect_uri: https://attune.example.com/auth/callback
    scopes: [groups]
    device_client:
      client_id: NATIVE_CLIENT_ID
      client_secret: null
```

For a custom server, replace the discovery URL with
`https://example.okta.com/oauth2/default/.well-known/openid-configuration` or the
chosen server ID. The configured discovery URL is used as supplied; Attune does
not independently select an issuer for the device client.

Helm chart 0.8.9 exposes `security.oidc.deviceClient.clientId` and
`security.oidc.requireGroups`. Chart 0.8.8 lacks those values and needs an
application-config or environment override such as
`ATTUNE__SECURITY__OIDC__DEVICE_CLIENT__CLIENT_ID`. Public native clients need no
device secret. Confidential client secrets belong in the API-only identity
Secret. See [deployment instructions](../deployment/oidc-device-login.md).

### Refresh and policy boundaries

Okta requires `offline_access` to return an Okta refresh token in its documented
device example. Attune does not store or use that provider refresh token; it issues
its own refresh token. `offline_access` is therefore optional for Attune's current
sign-in flow. Setting it in `oidc.scopes` requests it for both web and device
registrations. [D1]

Because Attune refresh is local, Okta provider-token revocation and refresh-token
policies do not automatically govern an already issued Attune refresh token.
That behavior must not be mistaken for continuing Okta-session validation.

Public-native setup matches the documented protocol, but an actual tenant test is
still needed for assignments, access policy, browser/MFA interaction, equal issuer
and subject, and native-app group delivery. Required proof-of-possession or token
encryption settings need implementation work before use.

Additional primary sources reviewed on 2026-10-01:

- [D1] [Okta device authorization guide](https://developer.okta.com/docs/guides/device-authorization-grant/main/).
- [D2] [Okta client authentication methods](https://developer.okta.com/docs/api/openapi/okta-oauth/guides/client-auth/).
- [D3] [Okta authorization servers](https://developer.okta.com/docs/concepts/auth-servers/).
- [D4] [Okta ID-token validation](https://developer.okta.com/docs/guides/validate-id-tokens/main/).
- [D5] [Okta key management and ID-token encryption](https://developer.okta.com/docs/guides/key-management/main/) and the rendered [client registration reference](https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/client/createclient).
- [D6] [Okta DPoP guide](https://developer.okta.com/docs/guides/dpop/main/), including its official [Markdown source](https://raw.githubusercontent.com/okta/okta-developer-docs/master/packages/@okta/vuepress-site/docs/guides/dpop/main/index.md).
- [D7] [Okta custom-domain and issuer configuration](https://developer.okta.com/docs/guides/custom-url-domain/main/).

[D1]: https://developer.okta.com/docs/guides/device-authorization-grant/main/
[D2]: https://developer.okta.com/docs/api/openapi/okta-oauth/guides/client-auth/
[D3]: https://developer.okta.com/docs/concepts/auth-servers/
[D4]: https://developer.okta.com/docs/guides/validate-id-tokens/main/
[D5]: https://developer.okta.com/docs/guides/key-management/main/
[D6]: https://developer.okta.com/docs/guides/dpop/main/
[D7]: https://developer.okta.com/docs/guides/custom-url-domain/main/

## Findings: claim delivery and shared web/native identities

Research date: 2026-10-01. This section covers groups, ID-token enrichment, app-specific claim configuration, and subject identifiers. Device-grant protocol mechanics and client-authentication methods belong to the separate investigation.

Evidence labels used below:

- **Documented** means the cited Okta documentation or OIDC specification states the behavior.
- **Source observation** means the current Attune working tree implements the behavior. Line references describe this snapshot, including uncommitted work.
- **Inference** means a conclusion drawn from those sources, rather than an explicit provider guarantee.
- **Unverified** means the result depends on tenant configuration or live tokens that this research did not inspect.

### Summary

Okta documents groups support on both org and custom authorization servers. The configuration location and token delivery differ. A native registration does not inherently prevent groups or authorization-server custom claims. Attune's shared identity design is consistent with Okta's documented public subject identifiers when both clients authenticate the same Okta user against the exact same issuer. That does not establish equal group claims or equal sign-in policy between the registrations. [O1][O2][O4][O5][O8][O9][S1]

The most direct implementation consequence is role replacement. In 0.7.4, requesting the `groups` scope or setting `require_groups: true` rejects missing groups before identity or role changes. Verified empty or narrower groups replace OIDC-managed roles on the shared identity, so successful authentication alone does not prove equal web/native authorization.

### Current Attune implementation

| Concern | Source observation |
| --- | --- |
| Shared configuration | [`OidcConfig`, `config.rs:448-456,480-494](../../crates/common/src/config.rs#L448) has one discovery URL, one main client ID, one extra-scopes list, and an optional device-client override. The override contains only a client ID and an optional secret. There is no separate device issuer or device-scopes field. |
| Requested claims | Browser login requests `openid email profile` plus `oidc.scopes` at [`oidc.rs:183-198`](../../crates/api/src/auth/oidc.rs#L183). Device login requests the same scope set at [`oidc_device.rs:181-188`](../../crates/api/src/auth/oidc_device.rs#L181). Neither path adds `groups` unless configuration supplies it. |
| Public native override | [`config.rs:490-494`](../../crates/common/src/config.rs#L490) uses `Option<String>` for the override secret. [`oidc_device.rs:49-64`](../../crates/api/src/auth/oidc_device.rs#L49) uses the override's own value. With an override and `client_secret: null`, it does not inherit the main client's secret. This is a configuration observation, not a provider-protocol finding. |
| Actual token audience | The device path passes the device client ID to `complete_provider_login` at [`oidc_device.rs:340-348`](../../crates/api/src/auth/oidc_device.rs#L340). That function validates the ID token against `token_client_id` at [`oidc.rs:335`](../../crates/api/src/auth/oidc.rs#L335). The verifier uses that client ID and the discovery issuer at [`oidc.rs:719-723`](../../crates/api/src/auth/oidc.rs#L719), with additional authorized-party checks at lines 755-770. |
| Canonical stored client | [`oidc.rs:342-353`](../../crates/api/src/auth/oidc.rs#L342) preserves the verified issuer and subject, stores the main web-client ID as `client_id`, and records a differing actual client as `authentication_client_id`. The stored main client ID is Attune identity metadata, not a claim that the native ID token had the web audience. |
| UserInfo enrichment | `complete_provider_login` in [`oidc.rs`](../../crates/api/src/auth/oidc.rs) uses the actual token client and the verified ID-token subject for UserInfo. A failed request cannot satisfy required groups. Verified ID-token groups can satisfy that requirement without UserInfo. |
| Merge rules | `extract_groups_from_claims` and `merge_userinfo_claims` in [`oidc.rs`](../../crates/api/src/auth/oidc.rs) accept a string or an array containing only strings under exactly `groups`. UserInfo fills absent groups, but cannot replace an explicit empty ID-token list. Missing and null groups remain absent; malformed values reject login. |
| Identity matching | [`oidc.rs:648-680`](../../crates/api/src/auth/oidc.rs#L648) passes issuer, subject, and the canonical main client ID to the repository. [`identity.rs:429-443`](../../crates/common/src/repositories/identity.rs#L429) matches all three. The issuer/subject uniqueness-conflict path rejects a row bound to another canonical client at [`identity.rs:520-537`](../../crates/common/src/repositories/identity.rs#L520). |
| Role synchronization | `upsert_oidc_identity` in [`identity.rs`](../../crates/common/src/repositories/identity.rs) commits identity attributes and OIDC-managed roles in one transaction. Identity row locks serialize concurrent logins. Empty groups remove only OIDC-managed roles; group-name conflicts do not take ownership of other-source assignments. Frozen identities reject login and roll back the update. |

**Inference, high confidence:** canonicalizing `client_id` allows an approved native login to reach the browser identity only when the verified issuer and subject also match. The code does not translate different subject identifiers. It also does not use equal email addresses to link identities. Email is a preferred login label at [`oidc.rs:694-699`](../../crates/api/src/auth/oidc.rs#L694), with login-collision handling at [`identity.rs:487-497`](../../crates/common/src/repositories/identity.rs#L487).

### Groups scope and claim support depend on the authorization server

| Question | Org authorization server | Custom authorization server, including `default` |
| --- | --- | --- |
| Issuer | `https://{yourOktaDomain}`. | `https://{yourOktaDomain}/oauth2/{authorizationServerId}`. `default` is a custom server, not the org server. [O3] |
| `groups` scope support | Okta lists `groups` as a reserved scope available on the org server. Its org discovery example includes `groups`. The org groups guide requests `openid groups`. [O1][O4][O8] | Okta's overview also lists `groups` as reserved and available on custom servers. This is documented support, not a guarantee that every tenant request passes its access policy. [O4] |
| Groups claim configuration | Configure the selected OIDC app's Sign On ID-token group filter or expression. The org groups guide documents groups in ID tokens, not org access tokens. [O1] | Configure a claim on the server's Claims tab. Claims can target ID tokens or access tokens, and can have scope conditions. ID-token inclusion can be set to Always. [O1][O2] |
| Arbitrary custom scopes and server-defined claims | The org server does not expose the custom-server scope and claim configuration mechanism. App-level groups are a documented exception to broad statements that org claims cannot be customized. [O1][O2][O3] | Custom scopes, claims, and access policies are supported. Production use requires the API Access Management product entitlement described by Okta. [O2][O3] |

**Documented distinction:** a scope named `groups` and a claim named `groups` are separate things. The custom-server groups example requests only `openid` for an access token containing a configured groups claim. A claim valid for Any scope does not necessarily require requesting `groups`. Conversely, requesting `groups` does not configure a claim, select its output name, or override a filter. [O1][O2]

**Confidence limit:** the custom-server discovery example in [O9] omits `groups`, while the reserved-scope text in [O4] explicitly says both server types support it. A sample's omission is not evidence of universal rejection. These sources also differ in how broadly they describe discovery publication of custom scopes. This research establishes documented scope support, not the accepted scope set or claim conditions of a particular custom server. [O3][O4][O6][O9]

**Inference for Attune:** its shared `oidc.scopes` list requires both registrations to be eligible for the requested scopes on the shared issuer. For org-server groups, the documented configuration is an app-level `groups` claim plus the requested `groups` scope. For a custom server, the relevant settings are the claim's exact name, token type, scope conditions, and applicable access policies. Configuring only an access-token claim named `Groups` or `GroupsClaim` does not satisfy Attune's ID-token/UserInfo reader, which recognizes lowercase `groups`. [O1][O2] See the merge and scope code above.

### Thin org-server ID tokens are flow-specific

**Documented, high confidence:** the org groups guide distinguishes these cases. [O1]

- An implicit request for only `id_token` produces a fat ID token with requested groups.
- Requests returning both ID and access tokens produce thin ID tokens for the flows the guide names: Implicit, Interaction Code, Resource Owner Password, and SAML 2.0 Assertion. Profile attributes and groups are absent even when their scopes are requested. The guide directs callers to UserInfo for groups.
- Authorization Code and Authorization Code with PKCE return both tokens, but the guide explicitly calls the org-server ID token fat and says groups are included.

**Unverified:** that passage does not name Device Authorization. It cannot establish whether a device-issued org ID token is thin or fat. The precise device-token claim set needs tenant evidence. No device-grant protocol conclusion is drawn here.

**Source observation:** the comment at [`oidc.rs:108-114`](../../crates/api/src/auth/oidc.rs#L108) describes thin Okta tokens when an access token is also returned. Taken as a rule for every flow, that wording is broader than Okta's documented distinction. The implementation itself calls UserInfo regardless of token thickness, so enrichment is useful without relying on that generalization.

**Documentation limit:** [O4]'s overview uses response-type tables and broad statements about claims moving to UserInfo when access tokens are issued. Those tables do not describe the final device-token payload. The dedicated groups guide provides the explicit Authorization Code/PKCE exception. Neither source proves the device case. Their thin-token terminology also differs on exactly which profile fields remain, so "thin" should not substitute for checking individual claims.

### UserInfo returns authorized claims, not another app's claim configuration

**Documented, high confidence:** Okta exposes these endpoints. Both require the OIDC `openid` scope and return information about the user represented by the access token. [O7][O10]

- Org server: `/oauth2/v1/userinfo`.
- Custom server: `/oauth2/{authorizationServerId}/v1/userinfo`.

Okta says UserInfo returns the full set of claims for the requested scopes even when the token omits some of them. Its overview ties available UserInfo claims to scopes associated with the access token. For groups, the documented scope-dependent claim is the user's memberships that also match the client app's ID-token group filter. "Full set" therefore does not mean every group in the directory or arbitrary claims from a different registration. [O1][O4]

**Documented OIDC requirement:** UserInfo `sub` must exactly match the ID-token `sub` before the client uses its values. Requested claims may also be omitted for privacy reasons. This bounds a general claim-delivery guarantee beyond Okta's own description. [S2]

**Source observation:** Attune supplies the expected subject to the OIDC library in `complete_provider_login`. It retains any explicit ID-token group list, including an empty one, and uses UserInfo only when that claim is absent. A missing or failed enrichment cannot satisfy required groups.

**Inference, high confidence:** the main web-client ID stored in Attune cannot make native-token UserInfo return the web app's groups. The provider grant and actual client configuration determine those claims. Missing native group configuration is therefore not repaired merely by identity canonicalization. [O1][O4]

**Source observation for 0.7.4:** if enrichment fails and required ID-token groups are absent, login rejects before persistence. If groups are optional, an absent claim produces no OIDC-managed roles. A verified empty or narrower group list removes OIDC-managed roles absent from that list, but preserves other-source assignments. These are Attune rules, not Okta guarantees.

### Native versus web registration is not the custom-claim boundary

**Documented:** Okta's app-integration guide supports web, native, and single-page OIDC apps. After its platform-specific settings, its common optional-settings section documents an ID-token group filter on the Sign On tab. The org groups guide describes selecting the OIDC app to configure, without a web-only restriction. [O1][O5]

**Documented:** authorization-server custom claims live on the custom server's Claims tab. Their conditions include token type, Always versus requested inclusion, scope, and expression or group filter. Okta's expressions can reference the current `app`, `app.clientId`, `app.profile`, and app-user attributes. This claim mechanism is described for OIDC clients, not as a web-only feature. [O2][O6][O11]

**Inference, high confidence:** a native app can receive custom-server claims when the claim and policy conditions match. A public native override's null secret does not itself establish a claim restriction. The registration still needs the intended user assignment, claim settings, and policy eligibility. Custom-server policies can target All clients or named clients, and their rules constrain users, grant types, and scopes. A rule covering only the web client does not automatically cover the native registration. [O4][O6]

**Inference:** equal requested scopes and a shared authorization server do not guarantee equal claims. App-specific expressions and allowlists can yield different groups or profile fields for the two registrations. [O1][O2][O11]

**Documentation limit:** the broad authorization-server capability table says org ID tokens can have custom claims, while the custom-claims guide restricts its server-defined custom-claim mechanism to custom servers. The groups exception is explicit. The guide also links a separate federated-entitlement claims feature. These statements do not justify a blanket claim that every possible org-server ID-token customization is forbidden, nor that arbitrary custom-server claims can be configured on any app's Sign On tab. [O1][O2][O3]

### Can the same user's `sub` be relied on across the web and native clients?

**Documented, high confidence within the stated subject policy:** both Okta org and custom OIDC metadata references say that valid subject types include `pairwise` and `public`, but "only public is currently supported." Their examples advertise `subject_types_supported: ["public"]`. OIDC defines public subjects as providing the same `sub` value to all clients. Pairwise subjects instead depend on the client or sector. [O8][O9][S1]

**Conclusion:** matching validated ID-token `iss` and `sub` across separate web and native client IDs is supported by Okta's documented public-subject behavior for the same Okta user and exact issuer. This conclusion combines an Okta statement with the normative definition of `public`; it is stronger than an inference from sample `00u...` values alone. Different `aud` values are expected for different clients and do not imply different users. [O4][O8][O9][S1]

The conclusion has these limits:

- **Documented:** the org issuer and a custom-server issuer are different. A common Okta tenant or hostname is not enough. App issuer settings can also select the org domain, custom domain, or a dynamic domain based on the request. [O3][O5]
- **Documented:** OIDC makes the ID-token issuer/subject pair the stable identity key. Email, name, and preferred username are not reliable linking keys. [S3]
- **Documented:** Okta's custom-server guide says the default subject-claim mapping can be edited. That passage does not clearly identify every ID-token versus access-token consequence. Its token examples show an ID-token `sub` shaped like an Okta user ID and an access-token `sub` containing a username. An access-token subject mapping is not evidence that an ID-token subject differs across clients. [O2][O6]
- **Unverified:** this research did not inspect the tenant's discovery responses, subject mapping, client registrations, token customization, or token pairs. Any app-dependent subject policy or customization that changes the actual ID-token subject would invalidate Attune's equality assumption. That possibility is a configuration boundary to check, not a finding that Okta supports pairwise subjects today.
- **Inference:** because Attune's OIDC configuration is provider-generic, the Okta result must not become a universal assumption about other issuers. A provider using pairwise subjects needs its own cross-client linking policy. The current canonicalization has no such translation. [S1] See `oidc.rs:342-353,664-674`.

For the documented Okta public-subject policy, the shared identity design is justified. Compatibility still depends on equal role-bearing groups and sufficient native-app policy, rather than subject equality alone.

### Tenant evidence still needed

These are unresolved verification targets, not results of a live tenant test:

1. The web and native ID tokens for the same assigned user have exactly equal `iss` and `sub`, and their respective client IDs in the intended audiences. The actual discovery metadata confirms the documented public-subject policy.
2. The native device-issued ID token's actual profile and groups payload is known. The reviewed thin-token guide does not establish that payload.
3. If groups are absent from that ID token, UserInfo for its access token supplies the intended lowercase `groups` claim and a matching subject.
4. Browser and native effective group sets agree for the Attune roles that must survive either login. Filters, expressions, source directories, and allowlists are accounted for on both registrations or in the custom-server claim.
5. The shared requested scopes and relevant user/client combination pass applicable access policies. Scope support in documentation is not proof of a matching tenant rule.

### Primary sources

All sources were read on 2026-10-01. Okta endpoint references that render only a title through a static fetch were read in their rendered official pages.

- [O1] [Okta, Customize tokens returned from Okta with a groups claim](https://developer.okta.com/docs/guides/customize-tokens-groups-claim/main/), especially the org-server request notes and custom-server claim setup.
- [O2] [Okta, Customize tokens returned from Okta with custom claims](https://developer.okta.com/docs/guides/customize-tokens-returned-from-okta/main/), especially Add a custom claim to a token and Include app-specific information in a custom claim.
- [O3] [Okta, Authorization servers](https://developer.okta.com/docs/concepts/auth-servers/), issuer boundaries, discovery, capability table, and product entitlement.
- [O4] [Okta, OpenID Connect & OAuth 2.0 overview](https://developer.okta.com/docs/api/openapi/okta-oauth/guides/overview/), Reserved scopes, Custom claims, Base claims, Scope-dependent claims, and Subtle behavior.
- [O5] [Okta Identity Engine, Create OpenID Connect app integrations](https://help.okta.com/oie/en-us/content/topics/apps/apps_app_integration_wizard_oidc.htm), app platforms, common optional group-filter settings, and issuer selection.
- [O6] [Okta, Create an authorization server](https://developer.okta.com/docs/guides/customize-authz-server/main/), client-targeted policies, user/scope rules, and claim mapping.
- [O7] [Okta org authorization server, UserInfo](https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/orgas/userinfo), endpoint and response semantics.
- [O8] [Okta org authorization server, Retrieve the OpenID Connect metadata](https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/orgas/getwellknownopenidconfiguration), `subject_types_supported` description and example scope list.
- [O9] [Okta custom authorization server, Retrieve the OpenID Connect metadata](https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/customas/getwellknownopenidconfigurationcustomas), `subject_types_supported` description and example scope list.
- [O10] [Okta custom authorization server, UserInfo](https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/customas/userinfocustomas), endpoint and response semantics.
- [O11] [Okta Expression Language](https://developer.okta.com/docs/reference/okta-expression-language/#expressions-for-oauth-2-0-oidc-custom-claims), in-context app references and app-user profiles.
- [S1] [OpenID Connect Core 1.0, section 8, Subject identifier types](https://openid.net/specs/openid-connect-core-1_0.html#SubjectIDTypes).
- [S2] [OpenID Connect Core 1.0, section 5.3.2, Successful UserInfo response](https://openid.net/specs/openid-connect-core-1_0.html#UserInfoResponse).
- [S3] [OpenID Connect Core 1.0, section 5.7, Claim stability and uniqueness](https://openid.net/specs/openid-connect-core-1_0.html#ClaimStability).

[O1]: https://developer.okta.com/docs/guides/customize-tokens-groups-claim/main/
[O2]: https://developer.okta.com/docs/guides/customize-tokens-returned-from-okta/main/
[O3]: https://developer.okta.com/docs/concepts/auth-servers/
[O4]: https://developer.okta.com/docs/api/openapi/okta-oauth/guides/overview/
[O5]: https://help.okta.com/oie/en-us/content/topics/apps/apps_app_integration_wizard_oidc.htm
[O6]: https://developer.okta.com/docs/guides/customize-authz-server/main/
[O7]: https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/orgas/userinfo
[O8]: https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/orgas/getwellknownopenidconfiguration
[O9]: https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/customas/getwellknownopenidconfigurationcustomas
[O10]: https://developer.okta.com/docs/api/openapi/okta-oauth/oauth/customas/userinfocustomas
[O11]: https://developer.okta.com/docs/reference/okta-expression-language/#expressions-for-oauth-2-0-oidc-custom-claims
[S1]: https://openid.net/specs/openid-connect-core-1_0.html#SubjectIDTypes
[S2]: https://openid.net/specs/openid-connect-core-1_0.html#UserInfoResponse
[S3]: https://openid.net/specs/openid-connect-core-1_0.html#ClaimStability
