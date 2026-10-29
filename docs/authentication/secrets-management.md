# Secrets Management in Attune Worker Service

## Overview

The Attune Worker Service includes a robust secrets management system that securely stores, retrieves, and injects secrets into action execution environments. Secrets are encrypted at rest in the database and decrypted on-demand during execution.

## Architecture

### Components

1. **SecretManager** (`crates/worker/src/secrets.rs`)
   - Core component responsible for secret operations
   - Handles fetching, decryption, and environment variable preparation
   - Integrated into `ActionExecutor` for seamless secret injection

2. **Database Storage** (`attune.key` table)
   - Stores secrets with ownership scoping (system, pack, action, sensor, identity)
   - Supports both encrypted and plaintext values
   - Tracks encryption key hash for validation

3. **Encryption System**
   - Uses **AES-256-GCM** for authenticated encryption
   - Derives encryption key from configured password using SHA-256
   - Generates random nonces for each encryption operation

## Secret Ownership Hierarchy

Secrets are organized in a hierarchical ownership model with increasing specificity:

### 1. System-Level Secrets
- **Owner Type**: `system`
- **Scope**: Available to all actions across all packs
- **Use Case**: Global configuration (API endpoints, common credentials)

### 2. Pack-Level Secrets
- **Owner Type**: `pack`
- **Scope**: Available to all actions within a specific pack
- **Use Case**: Pack-specific credentials, service endpoints

### 3. Action-Level Secrets
- **Owner Type**: `action`
- **Scope**: Available only to a specific action
- **Use Case**: Action-specific credentials, sensitive parameters

### Override Behavior

When an action is executed, secrets are fetched in the following order:
1. System secrets
2. Pack secrets (override system secrets with same name)
3. Action secrets (override pack/system secrets with same name)

This allows for flexible secret management where more specific secrets override less specific ones.

## Encryption Format

### Encrypted Value Format
```
Base64(nonce || ciphertext || authentication_tag)
```

This is one Base64 string with no separator:
- **Nonce**: 12-byte random value (96 bits) for AES-GCM
- **Ciphertext**: Encrypted payload followed by a 16-byte authentication tag

### Shared recorded-field encryption key derivation

Key records themselves now use ID-bound HKDF-SHA-256 and AES-GCM authenticated
context through `crypto::keystore`. The following shared derivation applies to
recorded fields and other generic encrypted values, not Key rows. See
[fingerprint remediation](../deployment/encryption-fingerprint-remediation.md)
for the exact per-record contract and offline conversion limits.

The AES key is derived from the configured server secret using SHA-256:

```
aes_key = SHA256(configured_secret_utf8)
```

This produces a 32-byte key. The configured secret must contain at least 256 bits
of cryptographically random entropy. A password or a length check alone does not
meet that requirement.

### Key Hash Validation

Each encrypted secret can optionally store the hash of the encryption key used to encrypt it:

```
key_hash = hex(SHA256(b"attune:encryption-key-fingerprint\0" || aes_key))
```

This allows validation without storing AES key material. Earlier fingerprints
were the shared AES key itself. Existing Key ciphertext and historical shared
metadata require coordinated conversion. See
[encryption fingerprint remediation](../deployment/encryption-fingerprint-remediation.md)
for storage coverage, rollout blockers and incident response.

## Configuration

### Security Configuration

Add to your `config.yaml`:

```yaml
security:
  # Encryption key for secrets (REQUIRED for encrypted secrets)
  encryption_key: "your-secret-encryption-password-here"
  
  # Or use environment variable
  # ATTUNE__SECURITY__ENCRYPTION_KEY=your-secret-encryption-password-here
```

⚠️ **Important Security Notes:**
- The encryption key should be a strong, random password (minimum 32 characters recommended)
- Store the encryption key securely (e.g., using a secrets manager, not in version control)
- If the encryption key is lost, encrypted secrets cannot be recovered
- Changing the encryption key requires re-encrypting all secrets

### Environment Variables

Override configuration via environment variables:

```bash
export ATTUNE__SECURITY__ENCRYPTION_KEY="your-encryption-key"
```

## Usage Examples

### Storing Secrets (via API)

#### System-Level Secret
```bash
curl -X POST http://localhost:8080/api/v1/keys \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "system.api_endpoint",
    "owner_type": "system",
    "name": "api_endpoint",
    "value": "https://api.example.com",
    "encrypted": false
  }'
```

#### Pack-Level Secret (Encrypted)
```bash
curl -X POST http://localhost:8080/api/v1/keys \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "mypack.api_key",
    "owner_type": "pack",
    "owner_pack": 1,
    "name": "api_key",
    "value": "sk_live_abc123def456",
    "encrypted": true
  }'
```

#### Action-Level Secret
```bash
curl -X POST http://localhost:8080/api/v1/keys \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "mypack.myaction.oauth_token",
    "owner_type": "action",
    "owner_action": 42,
    "name": "oauth_token",
    "value": "ya29.a0AfH6SMBx...",
    "encrypted": true
  }'
```

### Accessing Secrets in Actions

Secrets are automatically injected as environment variables during execution. The secret name is converted to uppercase and prefixed with `SECRET_`.

#### Python Action Example

```python
#!/usr/bin/env python3
import os

# Access secrets via environment variables
api_key = os.environ.get('SECRET_API_KEY')
db_password = os.environ.get('SECRET_DB_PASSWORD')
oauth_token = os.environ.get('SECRET_OAUTH_TOKEN')

if not api_key:
    print("Error: SECRET_API_KEY not found")
    exit(1)

# Use the secrets
print(f"Connecting to API with key: {api_key[:8]}...")
```

#### Shell Action Example

```bash
#!/bin/bash

# Access secrets
echo "API Key: ${SECRET_API_KEY:0:8}..."
echo "Database: ${SECRET_DB_HOST}"

# Use in commands
curl -H "Authorization: Bearer $SECRET_API_TOKEN" \
     https://api.example.com/data
```

### Environment Variable Naming Rules

Secret names are transformed as follows:
- Prefix: `SECRET_`
- Convert to uppercase
- Replace hyphens with underscores

Examples:
- `api_key` → `SECRET_API_KEY`
- `db-password` → `SECRET_DB_PASSWORD`
- `oauth_token` → `SECRET_OAUTH_TOKEN`

## Security Best Practices

### 1. Encryption Key Management
- **Generate Strong Keys**: Use at least 32 random characters
- **Secure Storage**: Store in a secrets manager (AWS Secrets Manager, HashiCorp Vault, etc.)
- **Rotation**: Plan for key rotation (requires re-encrypting all secrets)
- **Backup**: Keep encrypted backup of the encryption key

### 2. Secret Storage
- **Always Encrypt Sensitive Data**: Use `encrypted: true` for passwords, tokens, API keys
- **Plaintext for Non-Sensitive**: Use `encrypted: false` for URLs, usernames, configuration
- **Least Privilege**: Use action-level secrets for the most sensitive data

### 3. Action Development
- **Never Log Secrets**: Avoid printing secret values in action output
- **Mask in Errors**: Don't include secrets in error messages
- **Clear After Use**: In long-running processes, clear secrets from memory when done

### 4. Access Control
- **RBAC**: Limit who can create/read secrets using Attune's RBAC system
- **Audit Logging**: Enable audit logging for secret access (future feature)
- **Regular Reviews**: Periodically review and rotate secrets

## Implementation Details

### Encryption Process

```rust
// 1. Derive AES key from a cryptographically random server secret
let key = SHA256(configured_secret_utf8);

// 2. Generate random nonce
let nonce = random_bytes(12);

// 3. Encrypt plaintext
let ciphertext = AES256GCM.encrypt(key, nonce, plaintext);

// 4. Encode once; ciphertext includes the authentication tag
let encrypted_value = base64(concat(nonce, ciphertext));
```

### Decryption Process

```rust
// 1. Decode once, then split the first 12 bytes from ciphertext and tag
let bytes = base64_decode(encrypted_value);
let (nonce, ciphertext) = bytes.split_at(12);

// 2. Validate encryption key hash (if present)
let encryption_key = SHA256(configured_secret_utf8);
if key_hash != hex(SHA256(concat(b"attune:encryption-key-fingerprint\0", encryption_key))) {
    return Error("Key mismatch");
}

// 3. Decrypt ciphertext
let plaintext = AES256GCM.decrypt(encryption_key, nonce, ciphertext);
```

### Secret Injection Flow

```
1. ActionExecutor prepares execution context
2. SecretManager fetches secrets for action
   a. Query system-level secrets
   b. Query pack-level secrets
   c. Query action-level secrets
   d. Merge with later overriding earlier
3. Decrypt encrypted secrets
4. Transform to environment variables
5. Inject into execution context
6. Action executes with secrets available
```

## Troubleshooting

### "No encryption key configured"
**Problem**: Worker service cannot decrypt secrets.

**Solution**: Set the encryption key in configuration:
```yaml
security:
  encryption_key: "your-encryption-key-here"
```

### "Encryption key hash mismatch"
**Problem**: The encryption key used to decrypt doesn't match the key used to encrypt.

**Solution**: 
- Verify you're using the correct encryption key
- Check if encryption key was recently changed
- May need to re-encrypt secrets with new key

### "Decryption failed"
**Problem**: Secret cannot be decrypted.

**Causes**:
- Wrong encryption key
- Corrupted encrypted value
- Invalid format

**Solution**:
- Verify encryption key is correct
- Check secret value format, one Base64 string encoding nonce, ciphertext and tag
- Try re-encrypting the secret

### Secrets Not Available in Action
**Problem**: Environment variables like `SECRET_API_KEY` are not set.

**Checklist**:
- Verify secret exists in database with correct owner type
- Check secret name matches expected format
- Ensure action's pack has access to the secret
- Check worker logs for "Failed to fetch secrets" warnings

## API Reference

### SecretManager Methods

#### `fetch_secrets_for_action(action: &Action) -> Result<HashMap<String, String>>`
Fetches all secrets relevant to an action (system + pack + action level).

#### `encrypt_value(plaintext: &str) -> Result<String>`
Encrypts a plaintext value using the configured encryption key.

#### `prepare_secret_env(secrets: &HashMap<String, String>) -> HashMap<String, String>`
Transforms secret names to environment variable format.

## Future Enhancements

### Planned Features
- [ ] Secret versioning and rollback
- [ ] Audit logging for secret access
- [ ] Integration with external secret managers (Vault, AWS Secrets Manager)
- [ ] Automatic secret rotation
- [ ] Secret expiration and TTL
- [ ] Multi-key encryption (key per pack/action)
- [ ] Secret templates and inheritance

### Under Consideration
- [ ] Dynamic secret generation
- [ ] Just-in-time secret provisioning
- [ ] Secret usage analytics
- [ ] Integration with certificate management

## References

- [AES-GCM Encryption](https://en.wikipedia.org/wiki/Galois/Counter_Mode)
- [NIST SP 800-38D](https://csrc.nist.gov/publications/detail/sp/800-38d/final) - Recommendation for Block Cipher Modes of Operation: Galois/Counter Mode (GCM)
- [Key Management Best Practices](https://www.owasp.org/index.php/Key_Management_Cheat_Sheet)
