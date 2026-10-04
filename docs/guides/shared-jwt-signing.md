# Share a JWT signing key across packs

Use the API's approved signing operation when several packs need external JWT assertions without receiving a shared private key.
This guide uses Salesforce login assertions and separate subjects for `sales` and `support`.

## Store the signing profiles

Create an encrypted system key with local ref `salesforce_signer` through the authenticated key API.
Supply the private key and profiles as the key's structured `value`.
Use your operator credentials for this step.

```json
{
	"private_key_pem": "<RSA private key PEM>",
	"profiles": {
		"sales": {
			"issuer": "<approved Salesforce client ID>",
			"audience": "https://login.salesforce.com",
			"subjects": ["sales@example.com"],
			"pack_refs": ["sales"],
			"sensor_refs": ["sales.poll"],
			"max_ttl_seconds": 120
		},
		"support": {
			"issuer": "<approved Salesforce client ID>",
			"audience": "https://login.salesforce.com",
			"subjects": ["support@example.com"],
			"pack_refs": ["support"],
			"max_ttl_seconds": 120
		}
	}
}
```

Set `owner_type` to `system` and `encrypted` to `true` in the create-key request.
The resulting canonical ref is `system.salesforce_signer`.
See the [key API](../api/api-secrets.md) for the request envelope.

Keep each pack's approved subject in its own profile.
If a profile lists several packs and subjects, every listed pack can request every listed subject.
For a sensor, include its exact ref in `sensor_refs` as well as its pack in `pack_refs`.

The signer fixes the algorithm to RS256.
The configured maximum lifetime must be between 1 and 300 seconds.

## Grant operation access

Create an operator-managed permission set containing this grant:

```json
{
	"resource": "keys",
	"actions": ["use"],
	"constraints": {
		"owner_types": ["system"],
		"refs": ["system.salesforce_signer"]
	}
}
```

Assign the set to identities that run the approved pack actions.
Reference that set in each action's `default_execution_permission_set_refs`, or select it when creating an execution.
The registering identity must hold enough authority to attach the set.
Existing scoped `keys:read` grants also permit this operation.

For a managed sensor, grant operation access to the attributed pack installer.
The signer also checks the sensor's current workload fence and its profile allowlist.

## Request an assertion from action code

Call `POST /api/v1/keys/system.salesforce_signer/sign-jwt` with the execution's API token.
Use this request body for the sales profile:

```json
{
	"profile_ref": "sales",
	"subject": "sales@example.com",
	"ttl_seconds": 60
}
```

The response contains `data.assertion` and the Unix timestamp `data.expires_at`.
Exchange the assertion with Salesforce using the JWT bearer OAuth flow.
Keep action parameters in stdin JSON and use `ATTUNE_API_TOKEN` only for Attune authentication.

Mark any returned assertion or exchanged access token as secret in the action's output schema.
For example, an assertion field uses this flat schema:

```yaml
out_schema:
  assertion:
    type: string
    secret: true
```

Access tokens cannot call the signer directly.
Execution requests require a matching stored executor, action, and pinned executable.
Sensor requests require a matching active sensor and current workload assignment.

## Check the boundary

Request an unapproved subject or profile and verify that the API rejects it.
Request a lifetime above the profile maximum and verify that the API rejects it.
Retrieve the encrypted key with an identity that has read or use access but no decrypt grant.
Read access returns metadata with `value: null`; use-only access does not grant retrieval.
The signing operation returns an assertion without returning `private_key_pem`.

See [permission delegation and secret disclosure](../permissions/delegation-and-secret-disclosure.md) for historical execution disclosure rules.
