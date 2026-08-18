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
    auth_token: eyJhbGc...  # (stored securely)
    refresh_token: eyJhbGc...
    description: Staging environment
  production:
    api_url: https://api.example.com
    auth_token: eyJhbGc...
    refresh_token: eyJhbGc...
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

Authentication tokens are stored in plaintext in the config file. Ensure proper file permissions:

```bash
chmod 600 ~/.config/attune/config.yaml
```

**Future Enhancement**: OS keyring integration for secure token storage.

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

## Migration from Old Config

If you have an old single-server config, it will be migrated to a `default` profile on first run with the new version.

Old format:
```yaml
api_url: http://localhost:8080
auth_token: ...
refresh_token: ...
output_format: table
```

New format:
```yaml
current_profile: default
profiles:
  default:
    api_url: http://localhost:8080
    auth_token: ...
    refresh_token: ...
default_output_format: table
```

**Note**: Manual migration may be required. Back up your config before upgrading:

```bash
cp ~/.config/attune/config.yaml ~/.config/attune/config.yaml.backup
```

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
