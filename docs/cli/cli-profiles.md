# CLI Profile Management

The Attune CLI supports multiple named profiles, similar to SSH config or Kubernetes kubeconfig. This allows you to easily switch between different Attune servers (development, staging, production, etc.) without constantly changing configuration.

## Overview

Profiles are named configurations that store:
- API endpoint URL
- Authentication tokens (access and refresh)
- Output format preference (optional)
- Description (optional)

Each profile maintains its own authentication state, so you can be logged into multiple servers simultaneously.

## Configuration File

Profiles are stored in `~/.config/attune/config.yaml` (respects `$XDG_CONFIG_HOME`):

```yaml
current_profile: staging
profiles:
  default:
    api_url: http://localhost:8080
    description: Default local server
  staging:
    api_url: https://staging.example.com
    credential_encryption: true
    # credentials: <encrypted envelope written by attune auth login>
    description: Staging environment
  production:
    api_url: https://api.example.com
    credential_encryption: true
    # credentials: <encrypted envelope written by attune auth login>
    output_format: json
    description: Production environment
default_output_format: table
```

## Profile Commands

### List All Profiles

```bash
attune config profiles
```

Output:
```
Profiles
  • default
  • staging (current)
  • production
```

### Show Current Profile

```bash
attune config current
```

Output:
```
staging
```

### Add a Profile

```bash
attune config add-profile <name> --api-url <url> [--description <desc>]
```

Examples:
```bash
# Add staging profile
attune config add-profile staging \
  --api-url https://staging.example.com \
  --description "Staging environment"

# Add production profile
attune config add-profile production \
  --api-url https://api.example.com \
  --description "Production environment"

# Add local development profile with custom port
attune config add-profile dev \
  --api-url http://localhost:3000 \
  --description "Local development server"
```

### Switch Profile

```bash
attune config use <profile-name>
```

Example:
```bash
attune config use staging
# ✓ Switched to profile 'staging'
```

### Show Profile Details

```bash
attune config show-profile <profile-name>
```

Example:
```bash
attune config show-profile staging
```

Output:
```
Profile: staging
╭───────────────┬─────────────────────────────╮
│ Key           ┆ Value                       │
╞═══════════════╪═════════════════════════════╡
│ API URL       ┆ https://staging.example.com │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Auth Token    ┆ ***                         │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Refresh Token ┆ ***                         │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Description   ┆ Staging environment         │
╰───────────────┴─────────────────────────────╯
```

### Remove a Profile

```bash
attune config remove-profile <profile-name>
```

Example:
```bash
attune config remove-profile old-dev
# ✓ Profile 'old-dev' removed
```

**Note**: You cannot remove:
- The `default` profile
- The currently active profile (switch to another profile first)

## Using Profiles

### Runtime Profile Override

Use the `--profile` flag to temporarily use a different profile without switching:

```bash
# List packs on production without switching
attune --profile production pack list

# Execute action on staging
attune --profile staging action execute monitoring.check

# Get execution from different environment
attune --profile dev execution show 123
```

The `--profile` flag is available on all commands and does not change your current profile setting.

### Environment Variable

Set the `ATTUNE_PROFILE` environment variable:

```bash
export ATTUNE_PROFILE=staging
attune pack list  # Uses staging profile
```

### Precedence

When determining which profile to use:

1. `--profile` command-line flag (highest priority)
2. `ATTUNE_PROFILE` environment variable
3. `current_profile` in config file
4. `default` profile (fallback)

## Authentication Per Profile

Each profile maintains its own authentication state. You need to log in separately for each profile:

```bash
# Login to staging
attune config use staging
attune auth login --username admin

# Login to production
attune config use production
attune auth login --username admin

# Now you're authenticated to both environments
```

### Check Authentication Status

```bash
# Check current profile authentication
attune auth whoami

# Check specific profile
attune --profile staging auth whoami
attune --profile production auth whoami
```

### Logout from Profile

```bash
# Logout from current profile
attune auth logout

# Logout from specific profile
attune --profile staging auth logout
```

## Use Cases

### Development Workflow

```bash
# Setup profiles
attune config add-profile local --api-url http://localhost:8080
attune config add-profile staging --api-url https://staging.example.com
attune config add-profile prod --api-url https://api.example.com

# Work on local
attune config use local
attune pack install https://github.com/myorg/my-pack

# Test on staging
attune --profile staging pack list

# Deploy to production
attune --profile prod pack install https://github.com/myorg/my-pack --ref-spec v1.0.0
```

### Multi-Tenant Management

```bash
# Setup tenant profiles
attune config add-profile tenant-a --api-url https://tenant-a.example.com
attune config add-profile tenant-b --api-url https://tenant-b.example.com

# Login to each tenant
attune config use tenant-a
attune auth login --username admin

attune config use tenant-b
attune auth login --username admin

# Monitor both tenants
attune --profile tenant-a execution list --status failed
attune --profile tenant-b execution list --status failed
```

### CI/CD Integration

```bash
#!/bin/bash
# Deploy script for CI/CD

# Add ephemeral profile for CI environment
attune config add-profile ci --api-url "$CI_ATTUNE_API_URL"

# Login with CI credentials
attune --profile ci auth login \
  --username "$CI_USERNAME" \
  --password "$CI_PASSWORD"

# Deploy pack
attune --profile ci pack install "$PACK_REPO_URL" --ref-spec "$GIT_TAG"

# Verify deployment
attune --profile ci pack show "$PACK_NAME"

# Cleanup (optional - config is typically ephemeral in CI)
attune config remove-profile ci
```

### Scripting with Profiles

```bash
#!/bin/bash
# Check health across all environments

PROFILES=("dev" "staging" "production")

for profile in "${PROFILES[@]}"; do
  echo "Checking $profile..."
  
  # Execute health check
  result=$(attune --profile "$profile" \
    action execute core.health_check \
    --wait -j 2>/dev/null)
  
  if [ $? -eq 0 ]; then
    status=$(echo "$result" | jq -r '.status')
    echo "  $profile: $status"
  else
    echo "  $profile: ERROR"
  fi
done
```

## Profile Configuration Options

### Per-Profile Output Format

Set a default output format for a specific profile:

```bash
attune config use production
attune config set output_format json
```

Now all commands on the production profile will default to JSON output:

```bash
attune config use production
attune pack list  # Outputs JSON

attune config use staging
attune pack list  # Outputs table (default)
```

### Update Profile API URL

```bash
# Switch to profile
attune config use staging

# Update API URL
attune config set api_url https://new-staging.example.com

# Or update directly
attune config add-profile staging --api-url https://new-staging.example.com
```

## Best Practices

### 1. Use Descriptive Names

```bash
# Good
attune config add-profile staging-us-east --api-url ...
attune config add-profile prod-eu-west --api-url ...

# Avoid
attune config add-profile s1 --api-url ...
attune config add-profile p1 --api-url ...
```

### 2. Add Descriptions

```bash
attune config add-profile staging \
  --api-url https://staging.example.com \
  --description "Staging environment (US East)"
```

### 3. Keep Local as Default

Keep the `default` profile pointing to localhost for quick local testing:

```bash
# Default profile should be local
attune config use default
attune config get api_url
# http://localhost:8080
```

### 4. Profile Naming Convention

Use a consistent naming scheme:
- Environment-based: `dev`, `staging`, `prod`
- Region-based: `prod-us`, `prod-eu`, `prod-asia`
- Tenant-based: `tenant-acme`, `tenant-globex`
- Purpose-based: `local`, `ci`, `testing`

### 5. Verify Before Destructive Operations

Always verify you're on the correct profile before destructive operations:

```bash
# Check current profile
attune config current

# Or show API URL
attune config get api_url

# Then proceed
attune pack uninstall dangerous-pack
```

### 6. Use --profile for Queries

For read-only queries, use `--profile` instead of switching:

```bash
# Good - doesn't change current profile
attune --profile prod execution list

# Less ideal - changes current profile
attune config use prod
attune execution list
```

## Security Considerations

### Token Storage

CLI and MCP share the same profile credential store. Credential encryption is
**enabled by default**. Access and refresh tokens are encrypted together using
AES-256-GCM and a key derived with HKDF-SHA256 from local system identifiers and
a random per-profile salt. Each write uses a fresh encryption nonce. Refreshes
preserve the profile ID, salt, and enrolled identifier sources.

This reduces the usefulness of copying `config.yaml` alone. Identifiers are not
secret: this does not protect against captured identifiers, a clone with the
same identifiers, or access to the original user account. There is no OS keyring
or separate secret-key file in this mode.

The minimum binding is OS account identity, hostname, and machine identity.
Available CPU and system-volume identifiers are added when logging in. Only the
source names are saved; raw identifier values and keys are never saved or logged.
Sources are fixed for that login: later missing or changed sources require login
again; newly available optional sources do not change an existing binding.

- Linux/headless/WSL use the effective UID and OS username, hostname, and
  `/etc/machine-id`, with DMI product UUID as the explicit fallback source.
  CPU vendor/model and the root filesystem UUID are included when readable.
- Docker uses the same Linux provider. A persistent, instance-specific machine
  ID (or exposed DMI UUID), stable hostname, and stable account identity are
  required to persist encrypted credentials. Container recreation can require
  login again. Never bake a shared machine ID and credentials into an image.
- macOS uses UID/username, hostname, IOPlatformUUID, available CPU model, and
  root-volume UUID.
- Windows uses account SID/username, hostname, MachineGuid, available CPU model,
  and system-volume GUID.

Disk device paths, CPU frequency, load, and core counts are not key inputs.
Replacing hardware with identical reported identifiers is not detectable.

If credentials cannot be unlocked, log in again. Profile listings, login,
logout, and configuration management remain available without unlocking tokens.
Existing plaintext credentials without an explicit opt-out are removed at the
first normal configuration load; log in again after upgrading. Completion-only
reads do not rewrite the file. Attune creates no plaintext migration backup.

To explicitly opt a profile out if identifiers are unavailable or unstable:

```bash
# If its old encrypted credentials cannot be unlocked, clear them first.
attune --profile staging auth logout
attune --profile staging config set credential_encryption false
attune --profile staging auth login --username alice
```

The opt-out stores new tokens in **plaintext**, with owner-only file access.
Profile listings and configuration output identify encryption as disabled. If
the current encrypted credentials can be unlocked, disabling encryption preserves
them and rewrites them as plaintext. To enable encryption again:

```bash
attune --profile staging config set credential_encryption true
```

Enabling encryption immediately encrypts existing plaintext credentials. Failure
to obtain identifiers leaves the existing configuration intact; there is no
automatic fallback to plaintext. Make encryption-mode changes through the CLI,
not by manually changing YAML around an existing credential envelope.

Configuration writes use a cross-process lock and atomic replacement. Unix files
use mode `0600`; Windows files use a protected DACL granting the owner access.
Independent profile edits merge; conflicting changes to one profile fail with a
retry message. CLI and MCP serialize refreshes for the same profile, and token
rotation is saved back to its originating profile without changing the active
profile. Login retains its pre-authentication profile snapshot: a concurrent
change or deletion rejects the returned credentials rather than rebinding them
or falling back to another profile. A successful refresh whose persistence fails
reports an error.

Saved tokens are bound to the configured API endpoint. Changing the profile URL
clears its credentials; a different `--api-url` cannot reuse them. Supply runtime
credentials or log in to that endpoint. Explicit environment/argument credentials
are kept in memory, do not borrow a saved refresh token, and are not persisted
after refresh. Profile display output and login/refresh responses redact or omit
tokens, including JSON/YAML formats.

### Production Profiles

For production profiles:
1. Use strong passwords
2. Enable MFA (when available)
3. Regularly rotate credentials
4. Limit token lifetime
5. Use service accounts in CI/CD

### Profile Separation

Keep credentials separated:
- Use different usernames for different environments
- Don't share production credentials with staging
- Use least-privilege access per environment

## Troubleshooting

### Profile Not Found

```
Error: Profile 'staging' does not exist
```

**Solution**: Add the profile first:
```bash
attune config add-profile staging --api-url https://staging.example.com
```

### Cannot Remove Active Profile

```
Error: Cannot remove active profile 'staging'. Switch to another profile first.
```

**Solution**: Switch to another profile:
```bash
attune config use default
attune config remove-profile staging
```

### Authentication Failed

If authentication fails after switching profiles:

```bash
# Check if logged in
attune auth whoami

# If not, login
attune auth login --username your-username
```

### Wrong API URL

If commands are hitting the wrong server:

```bash
# Check current profile
attune config current

# Check API URL
attune config get api_url

# Switch if needed
attune config use correct-profile
```

## Upgrading Existing Credentials

Existing plaintext sessions require a fresh login after upgrading. Connection
URLs, descriptions, and other profile settings are retained. See
[Token Storage](#token-storage) for encryption behavior and explicit opt-out.

## Examples

### Complete Setup Example

```bash
# Setup local development
attune config add-profile local \
  --api-url http://localhost:8080 \
  --description "Local development"

# Setup staging
attune config add-profile staging \
  --api-url https://staging.example.com \
  --description "Staging environment"

# Setup production
attune config add-profile production \
  --api-url https://api.example.com \
  --description "Production environment"

# Login to each
for profile in local staging production; do
  attune config use "$profile"
  attune auth login --username admin
done

# Set production to always output JSON
attune config use production
attune config set output_format json

# Go back to local for development
attune config use local

# List all profiles
attune config profiles
```

### Profile Comparison Script

```bash
#!/bin/bash
# Compare pack versions across environments

PACK_NAME="monitoring"

echo "Pack: $PACK_NAME"
echo "---"

for profile in dev staging production; do
  version=$(attune --profile "$profile" pack show "$PACK_NAME" -j 2>/dev/null | jq -r '.version')
  
  if [ -n "$version" ] && [ "$version" != "null" ]; then
    echo "$profile: $version"
  else
    echo "$profile: Not installed"
  fi
done
```

## Related Commands

- `attune config list` - Show all configuration for current profile
- `attune config get <key>` - Get specific config value
- `attune config set <key> <value>` - Set config value for current profile
- `attune config path` - Show config file location
- `attune auth whoami` - Check authentication status

## See Also

- [CLI Documentation](cli.md)
- [Configuration Guide](../README.md#configuration)
- [Authentication](cli.md#authentication)
